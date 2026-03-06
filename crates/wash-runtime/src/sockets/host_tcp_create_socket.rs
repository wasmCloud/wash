use super::network::SocketResult;
use super::{SocketAddressFamily, TcpSocket, WasiSocketsCtxView};
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::{network::IpAddressFamily, tcp_create_socket};

type UpstreamTcpSocket = wasmtime_wasi::sockets::TcpSocket;

impl tcp_create_socket::Host for WasiSocketsCtxView<'_> {
    fn create_tcp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> SocketResult<Resource<UpstreamTcpSocket>> {
        let socket = TcpSocket::new(self.ctx, address_family.into())
            .map_err(super::network::socket_error_from_util)?;
        let socket = self.table.push(socket)?;
        Ok(Resource::new_own(socket.rep()))
    }
}

impl From<IpAddressFamily> for SocketAddressFamily {
    fn from(family: IpAddressFamily) -> SocketAddressFamily {
        match family {
            IpAddressFamily::Ipv4 => Self::Ipv4,
            IpAddressFamily::Ipv6 => Self::Ipv6,
        }
    }
}
