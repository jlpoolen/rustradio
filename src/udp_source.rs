// src/blocks/udp_source.rs
//
// 2025-07-07 ChatGPT
// $Header$
//
/*
Currently only support client mode
To build and pick up tracing:

    cargo build --features tokio-unstable

*/
use std::net::AddrParseError;
use std::net::Ipv4Addr;
use std::net::UdpSocket;


use anyhow::{Context};

use log::warn;

use socket2::{Socket, Domain, Type, Protocol};
//use std::net::{};
//use tracing::{info, debug, warn, error};

//use rustradio_macros::rustradio;
use crate::block::{Block, BlockRet, BlockEOF, BlockName};
use crate::stream::{ReadStream, WriteStream};
use crate::{Result, Sample};



#[derive(Debug, Clone)]
pub enum IpVersion {
    V4,
    V6,
}
#[derive(Debug, Clone)]
pub enum Platform {
    Linux,
    Windows,
    Default,
}

#[derive(Debug)]
pub enum MyError {
    AddrParse(AddrParseError),
    SocketError(std::io::Error),
    Other(String),
}
// https://doc.rust-lang.org/std/net/struct.UdpSocket.html does not break out port
// so bind_addr must always include port, you do not isolate address from port
// Likewise, for multicast_addr you have an IP and the port
// But for iface, you have just the address (used to register your subscription with server)
#[derive(Debug, Clone)]
pub struct UdpConfig<T: Sample> {
    pub bind_addr: String,          // e.g. "0.0.0.0:5000"
    pub multicast_addr: String,     // e.g. "239.192.0.1:5000" = IP + Port 
    pub iface_addr: Option<String>, // e.g. Some("192.168.1.X") = IP of current client
    pub reuse: Option<bool>,        // single or multicast
    pub ip_version: IpVersion,         
    pub platform: Platform,            
    _phantom: std::marker::PhantomData<T>,
}

// TCP: https://doc.rust-lang.org/std/net/struct.TcpStream.html
// Raw socket:  https://docs.rs/socket2/latest/socket2/index.html
// UDP: https://doc.rust-lang.org/std/net/struct.UdpSocket.html
// while this supports receive and send, the nature of the block architecture
// requires that it be one or the other, not both.  So you create a block to
// receive with, and/or you create another block to send
//
// While we are using std::net::UdpSocket, we're only doing so after creating
// a raw socket using socket2 which gives greater control over the socket and 
// allows for configuration of over domain, type, protocol.  We do use some
// structs from UdgSocket.  So we create a socket2
// with all the configurations and then cast the socket2 into a std::net::UdpSocket

// bind address: the specific IP address and port number that a UDP socket is associated with on the local machine. 
// This allows the socket to receive incoming UDP datagrams destined for that address and port. 
// Essentially, it's the local "listening address" for the UDP socket

// multicast address: a special IP address range (224.0.0.0 to 239.255.255.255 for IPv4) used to send a single packet 
// to multiple hosts simultaneously, without needing to know each individual recipient's address. Like a group mail server.

// interface address: refers to the specific network interface (like an Ethernet card or Wi-Fi adapter) 
// on a machine that a socket is bound to or intended to use for sending or receiving data. Example:  192.168.1.XX which
// is the current machine's IP.

// UdpSource Builder
#[derive(Debug, Clone)]
pub struct UdpSourceBuilder<T: Sample> {
    config: UdpConfig<T>,
}


impl<T: Sample + std::fmt::Debug> UdpSourceBuilder<T> {
    pub fn new(bind_addr: &str, multicast_addr: &str) -> Self {
        
        Self {
            config: UdpConfig {
                bind_addr: bind_addr.to_string(),
                multicast_addr: multicast_addr.to_string(),
                iface_addr: None,
                reuse: Some(true),
                ip_version: IpVersion::V4,   // or infer from bind_addr
                platform: Platform::Default, // no platform-specific logic yet
                _phantom: std::marker::PhantomData,
            },
        }
    }


    pub fn iface_addr(mut self, addr: &str) -> Self {
        self.config.iface_addr = Some(addr.to_string());
        self
    }

    pub fn reuse_addr(mut self, reuse: bool) -> Self {
        self.config.reuse = Some(reuse);
        self
    }

    pub fn build(self) -> anyhow::Result<(UdpSource<T>, ReadStream<T>)> {
        let domain = match self.config.ip_version {
            IpVersion::V4 => Domain::IPV4,
            IpVersion::V6 => Domain::IPV6,
        };
        match self.config.platform {
            Platform::Linux => {
                // maybe set SO_REUSEPORT if needed, using platform-specific extensions
            }
            Platform::Windows => {
                // disable multicast loopback, or whatever Windows might want
            }
            Platform::Default => {}
        }

        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))
            .context("Failed to create UDP socket")?;

        // Join multicast group if specified
        if !self.config.multicast_addr.is_empty() {
            let multi: Ipv4Addr = self
                .config
                .multicast_addr
                .parse()
                .context("Invalid multicast address")?;

            let iface: Ipv4Addr = self
                .config
                .iface_addr
                .as_deref()
                .unwrap_or("0.0.0.0")
                .parse()
                .context("Invalid interface address")?;

            socket
                .join_multicast_v4(&multi, &iface)
                .context("Failed to join multicast group")?;
        }

        if self.config.reuse.unwrap_or(false) {
            socket
                .set_reuse_address(true)
                .context("Failed to set reuse address")?;
        }

        socket
            .set_nonblocking(true)
            .context("Failed to set non-blocking mode")?;

        let (rx, tx) = crate::stream::new_stream();
        let udp_socket: std::net::UdpSocket = socket.into();
        let udp_source = UdpSource {
            socket: udp_socket,
            buffer: [0u8; 4096],
            dst: rx,
        };

        Ok((udp_source, tx))
    }
}

pub struct UdpSource<T: Sample> {
    dst: WriteStream<T>,
    socket: UdpSocket,
    buffer: [u8; 4096],
}

impl<T: Sample> BlockName for UdpSource<T> {
    fn block_name(&self) -> &str {
        "UdpSource"
    }
}

impl<T> Block for UdpSource<T>
where
    T: Sample<Type = T> + std::fmt::Debug,
{
 
    //fn work(&mut self, _io: &mut crate::runtime::IO) -> Result<crate::block::BlockRet> {
    fn work(&mut self) -> Result<BlockRet> {
        match self.socket.recv(&mut self.buffer) {
            Ok(n) => {
                let chunked = self.buffer[..n].chunks_exact(T::size());
                let remainder = chunked.remainder();

                for chunk in chunked {
                    match T::parse(chunk) {
                        Ok(sample) => {
                            if let Ok(mut writer) = self.dst.write_buf() {
                                writer.fill_from_slice(&[sample]);  // only if you have a full slice
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse sample: {:?}", e);
                        }
                    }
                }

                if !remainder.is_empty() {
                    warn!(
                        "Discarding {} leftover bytes (incomplete sample)",
                        remainder.len()
                    );
                }

            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No data available (non-blocking)
            }
            Err(_) => todo!()

            //Err(e) => {
            //    return Err(anyhow::anyhow!("UDP recv failed: {}", e));
            //}
        }
        
        Ok(crate::block::BlockRet::Again)
    }
}

//
// The user will kill the process
// TODO: handle when the socket dies 
//
impl<T: Sample> BlockEOF for UdpSource<T> {
    fn eof(&mut self) -> bool {
        false
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_receives_udp() -> Result<()> {
        // 1. Launch a thread that sends test data via UDP
        // 2. Instantiate UdpSourceBuilder and build the block
        // 3. Call work() a few times
        // 4. Read samples from the ReadStream<T>
        // 5. Assert expected results

        Ok(())
    }
}
