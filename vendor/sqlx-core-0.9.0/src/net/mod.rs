mod socket;
pub mod tls;

pub use socket::BufferedSocket;
pub use socket::Socket;
pub use socket::SocketIntoBox;
pub use socket::WithSocket;
pub use socket::WriteBuffer;
pub use socket::connect_tcp;
pub use socket::connect_uds;
