use super::network::SocketResult;
use wasmtime_wasi::p2::bindings::sockets::{network::IpAddressFamily, udp_create_socket};
use super::UdpSocket;
use super::WasiSocketsCtxView;
use wasmtime::component::Resource;

type UpstreamUdpSocket = wasmtime_wasi::sockets::UdpSocket;

impl udp_create_socket::Host for WasiSocketsCtxView<'_> {
    fn create_udp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> SocketResult<Resource<UpstreamUdpSocket>> {
        let socket = UdpSocket::new(self.ctx, address_family.into()).map_err(super::network::socket_error_from_util)?;
        let socket = self.table.push(socket)?;
        Ok(Resource::new_own(socket.rep()))
    }
}
