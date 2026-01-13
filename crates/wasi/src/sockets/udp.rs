use crate::runtime::with_ambient_tokio_runtime;
use crate::sockets::util::{
    ErrorCode, get_unicast_hop_limit, is_valid_address_family, is_valid_remote_address,
    receive_buffer_size, send_buffer_size, set_receive_buffer_size, set_send_buffer_size,
    set_unicast_hop_limit, udp_bind, udp_disconnect, udp_socket,
};
use crate::sockets::{SocketAddrCheck, SocketAddressFamily, WasiSocketsCtx};
use cap_net_ext::AddressFamily;
use io_lifetimes::AsSocketlike as _;
use io_lifetimes::raw::{FromRawSocketlike as _, IntoRawSocketlike as _};
use rustix::io::Errno;
use rustix::net::connect;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::debug;

/// The state of a UDP socket.
///
/// This represents the various states a socket can be in during the
/// activities of binding, and connecting.
#[derive(Clone)]
enum UdpState {
    /// The initial state for a newly-created socket.
    Default,

    /// A `bind` operation has started but has yet to complete with
    /// `finish_bind`.
    BindStarted,

    /// Binding finished via `finish_bind`. The socket has an address but
    /// is not yet listening for connections.
    Bound,

    /// The socket is "connected" to a peer address.
    #[cfg_attr(
        not(feature = "p3"),
        expect(dead_code, reason = "p2 has its own way of managing sending/receiving")
    )]
    Connected(SocketAddr),
}

/// A host UDP socket, plus associated bookkeeping.
///
/// The inner state is wrapped in an Arc because the same underlying socket is
/// used for implementing the stream types.
#[derive(Clone)]
pub struct NetworkUdpSocket {
    socket: Arc<tokio::net::UdpSocket>,

    /// The current state in the bind/connect progression.
    udp_state: UdpState,

    /// Socket address family.
    family: SocketAddressFamily,

    /// If set, use this custom check for addrs, otherwise use what's in
    /// `WasiSocketsCtx`.
    socket_addr_check: Option<SocketAddrCheck>,
}

impl NetworkUdpSocket {
    /// Create a new socket in the given family.
    fn new(cx: &WasiSocketsCtx, family: AddressFamily) -> Result<Self, ErrorCode> {
        cx.allowed_network_uses.check_allowed_udp()?;

        // Delegate socket creation to cap_net_ext. They handle a couple of things for us:
        // - On Windows: call WSAStartup if not done before.
        // - Set the NONBLOCK and CLOEXEC flags. Either immediately during socket creation,
        //   or afterwards using ioctl or fcntl. Exact method depends on the platform.

        let fd = udp_socket(family)?;

        let socket_address_family = match family {
            AddressFamily::Ipv4 => SocketAddressFamily::Ipv4,
            AddressFamily::Ipv6 => {
                rustix::net::sockopt::set_ipv6_v6only(&fd, true)?;
                SocketAddressFamily::Ipv6
            }
        };

        let socket = with_ambient_tokio_runtime(|| {
            tokio::net::UdpSocket::try_from(unsafe {
                std::net::UdpSocket::from_raw_socketlike(fd.into_raw_socketlike())
            })
        })?;

        Ok(Self {
            socket: Arc::new(socket),
            udp_state: UdpState::Default,
            family: socket_address_family,
            socket_addr_check: None,
        })
    }

    fn bind(&mut self, addr: SocketAddr) -> Result<(), ErrorCode> {
        udp_bind(&self.socket, addr)?;
        self.udp_state = UdpState::BindStarted;
        Ok(())
    }

    fn finish_bind(&mut self) -> Result<(), ErrorCode> {
        match self.udp_state {
            UdpState::BindStarted => {
                self.udp_state = UdpState::Bound;
                Ok(())
            }
            _ => Err(ErrorCode::NotInProgress),
        }
    }

    fn is_connected(&self) -> bool {
        matches!(self.udp_state, UdpState::Connected(..))
    }

    fn is_bound(&self) -> bool {
        matches!(self.udp_state, UdpState::Connected(..) | UdpState::Bound)
    }

    fn disconnect(&mut self) -> Result<(), ErrorCode> {
        if !self.is_connected() {
            return Err(ErrorCode::InvalidState);
        }
        udp_disconnect(&self.socket)?;
        self.udp_state = UdpState::Bound;
        Ok(())
    }

    fn connect(&mut self, addr: SocketAddr) -> Result<(), ErrorCode> {
        if !is_valid_address_family(addr.ip(), self.family) || !is_valid_remote_address(addr) {
            return Err(ErrorCode::InvalidArgument);
        }

        match self.udp_state {
            UdpState::Bound | UdpState::Connected(_) => {}
            _ => return Err(ErrorCode::InvalidState),
        }

        // We disconnect & (re)connect in two distinct steps for two reasons:
        // - To leave our socket instance in a consistent state in case the
        //   connect fails.
        // - When reconnecting to a different address, Linux sometimes fails
        //   if there isn't a disconnect in between.

        // Step #1: Disconnect
        if let UdpState::Connected(..) = self.udp_state {
            udp_disconnect(&self.socket)?;
            self.udp_state = UdpState::Bound;
        }
        // Step #2: (Re)connect
        connect(&self.socket, &addr).map_err(|error| match error {
            Errno::AFNOSUPPORT => ErrorCode::InvalidArgument, // See `udp_bind` implementation.
            Errno::INPROGRESS => {
                debug!("UDP connect returned EINPROGRESS, which should never happen");
                ErrorCode::Unknown
            }
            err => err.into(),
        })?;
        self.udp_state = UdpState::Connected(addr);
        Ok(())
    }

    #[cfg(feature = "p3")]
    fn send(&self, buf: Vec<u8>) -> impl Future<Output = Result<(), ErrorCode>> + use<> {
        let socket = if let UdpState::Connected(..) = self.udp_state {
            Ok(Arc::clone(&self.socket))
        } else {
            Err(ErrorCode::InvalidArgument)
        };
        async move {
            let socket = socket?;
            send(&socket, &buf).await
        }
    }

    #[cfg(feature = "p3")]
    fn send_to(
        &self,
        buf: Vec<u8>,
        addr: SocketAddr,
    ) -> impl Future<Output = Result<(), ErrorCode>> + use<> {
        enum Mode {
            Send(Arc<tokio::net::UdpSocket>),
            SendTo(Arc<tokio::net::UdpSocket>, SocketAddr),
        }
        let socket = match &self.udp_state {
            UdpState::BindStarted => Err(ErrorCode::InvalidState),
            UdpState::Default | UdpState::Bound => Ok(Mode::SendTo(Arc::clone(&self.socket), addr)),
            UdpState::Connected(caddr) if addr == *caddr => {
                Ok(Mode::Send(Arc::clone(&self.socket)))
            }
            UdpState::Connected(..) => Err(ErrorCode::InvalidArgument),
        };
        async move {
            match socket? {
                Mode::Send(socket) => send(&socket, &buf).await,
                Mode::SendTo(socket, addr) => send_to(&socket, &buf, addr).await,
            }
        }
    }

    #[cfg(feature = "p3")]
    fn receive(&self) -> impl Future<Output = Result<(Vec<u8>, SocketAddr), ErrorCode>> + use<> {
        enum Mode {
            Recv(Arc<tokio::net::UdpSocket>, SocketAddr),
            RecvFrom(Arc<tokio::net::UdpSocket>),
        }
        let socket = match self.udp_state {
            UdpState::Default | UdpState::BindStarted => Err(ErrorCode::InvalidState),
            UdpState::Bound => Ok(Mode::RecvFrom(Arc::clone(&self.socket))),
            UdpState::Connected(addr) => Ok(Mode::Recv(Arc::clone(&self.socket), addr)),
        };
        async move {
            let socket = socket?;
            let mut buf = vec![0; super::MAX_UDP_DATAGRAM_SIZE];
            let (n, addr) = match socket {
                Mode::Recv(socket, addr) => {
                    let n = socket.recv(&mut buf).await?;
                    (n, addr)
                }
                Mode::RecvFrom(socket) => {
                    let (n, addr) = socket.recv_from(&mut buf).await?;
                    (n, addr)
                }
            };
            buf.truncate(n);
            Ok((buf, addr))
        }
    }

    fn local_address(&self) -> Result<SocketAddr, ErrorCode> {
        if matches!(self.udp_state, UdpState::Default | UdpState::BindStarted) {
            return Err(ErrorCode::InvalidState);
        }
        let addr = self
            .socket
            .as_socketlike_view::<std::net::UdpSocket>()
            .local_addr()?;
        Ok(addr)
    }

    fn remote_address(&self) -> Result<SocketAddr, ErrorCode> {
        if !matches!(self.udp_state, UdpState::Connected(..)) {
            return Err(ErrorCode::InvalidState);
        }
        let addr = self
            .socket
            .as_socketlike_view::<std::net::UdpSocket>()
            .peer_addr()?;
        Ok(addr)
    }

    pub(crate) fn address_family(&self) -> SocketAddressFamily {
        self.family
    }

    fn unicast_hop_limit(&self) -> Result<u8, ErrorCode> {
        let n = get_unicast_hop_limit(&self.socket, self.family)?;
        Ok(n)
    }

    fn set_unicast_hop_limit(&self, value: u8) -> Result<(), ErrorCode> {
        set_unicast_hop_limit(&self.socket, self.family, value)?;
        Ok(())
    }

    fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        let n = receive_buffer_size(&self.socket)?;
        Ok(n)
    }

    fn set_receive_buffer_size(&self, value: u64) -> Result<(), ErrorCode> {
        set_receive_buffer_size(&self.socket, value)?;
        Ok(())
    }

    fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        let n = send_buffer_size(&self.socket)?;
        Ok(n)
    }

    fn set_send_buffer_size(&self, value: u64) -> Result<(), ErrorCode> {
        set_send_buffer_size(&self.socket, value)?;
        Ok(())
    }

    pub(crate) fn socket(&self) -> &Arc<tokio::net::UdpSocket> {
        &self.socket
    }

    pub(crate) fn socket_addr_check(&self) -> Option<&SocketAddrCheck> {
        self.socket_addr_check.as_ref()
    }

    fn set_socket_addr_check(&mut self, check: Option<SocketAddrCheck>) {
        self.socket_addr_check = check;
    }
}

#[cfg(feature = "p3")]
async fn send(socket: &tokio::net::UdpSocket, buf: &[u8]) -> Result<(), ErrorCode> {
    let n = socket.send(buf).await?;
    // From Rust stdlib docs:
    // > Note that the operating system may refuse buffers larger than 65507.
    // > However, partial writes are not possible until buffer sizes above `i32::MAX`.
    //
    // For example, on Windows, at most `i32::MAX` bytes will be written
    if n != buf.len() {
        Err(ErrorCode::Unknown)
    } else {
        Ok(())
    }
}

#[cfg(feature = "p3")]
async fn send_to(
    socket: &tokio::net::UdpSocket,
    buf: &[u8],
    addr: SocketAddr,
) -> Result<(), ErrorCode> {
    let n = socket.send_to(buf, addr).await?;
    // See [`send`] documentation
    if n != buf.len() {
        Err(ErrorCode::Unknown)
    } else {
        Ok(())
    }
}

impl super::loopback::UdpSocket {
    pub fn new(
        socket: &NetworkUdpSocket,
        state: super::loopback::UdpState,
    ) -> Result<Self, ErrorCode> {
        let hop_limit = get_unicast_hop_limit(&socket.socket, socket.family)?;

        let receive_buffer_size = receive_buffer_size(&socket.socket)?;

        let send_buffer_size = send_buffer_size(&socket.socket)?;
        let send_buffer_size = send_buffer_size
            .try_into()
            .unwrap_or(Self::MAX_SEND_BUFFER_SIZE);

        Ok(Self {
            state,
            hop_limit,
            receive_buffer_size,
            send_buffer_size,
            family: socket.family,
            socket_addr_check: socket.socket_addr_check.clone(),
        })
    }
}

pub enum UdpSocket {
    Network(NetworkUdpSocket),
    Loopback(super::loopback::UdpSocket),
    Unspecified {
        net: NetworkUdpSocket,
        lo: super::loopback::UdpSocket,
    },
}

impl UdpSocket {
    pub(crate) fn new(cx: &WasiSocketsCtx, family: AddressFamily) -> Result<Self, ErrorCode> {
        NetworkUdpSocket::new(cx, family).map(Self::Network)
    }

    pub(crate) fn bind(
        &mut self,
        mut addr: SocketAddr,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        use core::net::{Ipv4Addr, Ipv6Addr};

        let Self::Network(socket) = self else {
            return Err(ErrorCode::InvalidState);
        };
        if !matches!(socket.udp_state, UdpState::Default) {
            return Err(ErrorCode::InvalidState);
        }
        if !is_valid_address_family(addr.ip(), socket.family) {
            return Err(ErrorCode::InvalidArgument);
        }
        let ip = addr.ip().to_canonical();
        if !ip.is_loopback() {
            socket.bind(addr)?;
            if !ip.is_unspecified() {
                return Ok(());
            }
            addr = socket.socket.local_addr()?;
            match &mut addr {
                SocketAddr::V4(addr) => addr.set_ip(Ipv4Addr::LOCALHOST),
                SocketAddr::V6(addr) => addr.set_ip(Ipv6Addr::LOCALHOST),
            }
        };

        let (addr, rx) = loopback.bind_udp(addr)?;
        let lo = super::loopback::UdpSocket::new(
            socket,
            super::loopback::UdpState::BindStarted {
                local_address: addr,
                rx,
            },
        )?;

        if ip.is_unspecified() {
            *self = Self::Unspecified {
                net: socket.clone(),
                lo,
            }
        } else {
            *self = Self::Loopback(lo);
        }
        return Ok(());
    }

    pub(crate) fn finish_bind(&mut self) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.finish_bind(),
            Self::Loopback(socket) => socket.finish_bind(),
            Self::Unspecified { net, lo } => {
                net.finish_bind()?;
                lo.finish_bind()
            }
        }
    }

    pub(crate) fn is_connected(&self) -> bool {
        match self {
            Self::Network(socket) => socket.is_connected(),
            Self::Loopback(socket) => socket.is_connected(),
            Self::Unspecified { net, lo } => net.is_connected() && lo.is_connected(),
        }
    }

    pub(crate) fn is_bound(&self) -> bool {
        match self {
            Self::Network(socket) => socket.is_bound(),
            Self::Loopback(socket) => socket.is_bound(),
            Self::Unspecified { net, lo } => net.is_bound() && lo.is_bound(),
        }
    }

    pub(crate) fn disconnect(
        &mut self,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.disconnect(),
            Self::Loopback(socket) => socket.disconnect(loopback),
            Self::Unspecified { net, lo } => {
                net.disconnect()?;
                lo.disconnect(loopback)
            }
        }
    }

    pub(crate) fn connect(
        &mut self,
        addr: SocketAddr,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.connect(addr),
            Self::Loopback(socket) => socket.connect(addr, loopback),
            Self::Unspecified { net, lo } => {
                net.connect(addr)?;
                lo.connect(addr, loopback)
            }
        }
    }

    #[cfg(feature = "p3")]
    pub(crate) fn send(&self, buf: Vec<u8>) -> impl Future<Output = Result<(), ErrorCode>> + use<> {
        match self {
            Self::Network(socket) => socket.send(buf),
            Self::Loopback(..) | Self::Unspecified { .. } => todo!(),
        }
    }

    #[cfg(feature = "p3")]
    pub(crate) fn send_to(
        &self,
        buf: Vec<u8>,
        addr: SocketAddr,
    ) -> impl Future<Output = Result<(), ErrorCode>> + use<> {
        match self {
            Self::Network(socket) => socket.send_to(buf, addr),
            Self::Loopback(..) | Self::Unspecified { .. } => todo!(),
        }
    }

    #[cfg(feature = "p3")]
    pub(crate) fn receive(
        &self,
    ) -> impl Future<Output = Result<(Vec<u8>, SocketAddr), ErrorCode>> + use<> {
        match self {
            Self::Network(socket) => socket.receive(),
            Self::Loopback(..) | Self::Unspecified { .. } => todo!(),
        }
    }

    pub(crate) fn local_address(&self) -> Result<SocketAddr, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => socket.local_address(),
            Self::Loopback(socket) => socket.local_address(),
        }
    }

    pub(crate) fn remote_address(&self) -> Result<SocketAddr, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.remote_address()
            }
            Self::Loopback(socket) => socket.remote_address(),
        }
    }

    pub(crate) fn address_family(&self) -> SocketAddressFamily {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.address_family()
            }
            Self::Loopback(socket) => socket.address_family(),
        }
    }

    pub(crate) fn unicast_hop_limit(&self) -> Result<u8, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.unicast_hop_limit()
            }
            Self::Loopback(socket) => socket.unicast_hop_limit(),
        }
    }

    pub(crate) fn set_unicast_hop_limit(&mut self, value: u8) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_unicast_hop_limit(value),
            Self::Loopback(socket) => socket.set_unicast_hop_limit(value),
            Self::Unspecified { net, lo } => {
                net.set_unicast_hop_limit(value)?;
                lo.set_unicast_hop_limit(value)
            }
        }
    }

    pub(crate) fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.receive_buffer_size()
            }
            Self::Loopback(socket) => socket.receive_buffer_size(),
        }
    }

    pub(crate) fn set_receive_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_receive_buffer_size(value),
            Self::Loopback(socket) => socket.set_receive_buffer_size(value),
            Self::Unspecified { net, lo } => {
                net.set_receive_buffer_size(value)?;
                lo.set_receive_buffer_size(value)
            }
        }
    }

    pub(crate) fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.send_buffer_size()
            }
            Self::Loopback(socket) => socket.send_buffer_size(),
        }
    }

    pub(crate) fn set_send_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_send_buffer_size(value),
            Self::Loopback(socket) => socket.set_send_buffer_size(value),
            Self::Unspecified { net, lo } => {
                net.set_send_buffer_size(value)?;
                lo.set_send_buffer_size(value)
            }
        }
    }

    pub(crate) fn socket_addr_check(&self) -> Option<&SocketAddrCheck> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.socket_addr_check()
            }
            Self::Loopback(socket) => socket.socket_addr_check(),
        }
    }

    pub(crate) fn set_socket_addr_check(&mut self, check: Option<SocketAddrCheck>) {
        match self {
            Self::Network(socket) => socket.set_socket_addr_check(check),
            Self::Loopback(socket) => socket.set_socket_addr_check(check),
            Self::Unspecified { net, lo } => {
                net.set_socket_addr_check(check.clone());
                lo.set_socket_addr_check(check);
            }
        }
    }

    pub(crate) fn drop(self, loopback: &mut super::loopback::Network) -> wasmtime::Result<()> {
        match self {
            Self::Network(socket) => {
                drop(socket);
                Ok(())
            }
            Self::Loopback(socket) => socket.drop(loopback),
            Self::Unspecified { net, lo } => {
                drop(net);
                lo.drop(loopback)
            }
        }
    }
}
