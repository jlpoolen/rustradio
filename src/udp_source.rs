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
//use crate::circular_buffer::BufferReader;
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
// so bind_addr must always include port.  However, we're using socket2 and there
// are possible scenarios where you might have differing port numbers, so we'll break
// out port for bind & multicast and then join address and port as needed further on
#[derive(Debug, Clone)]
pub struct UdpConfig<T: Sample> {
    pub bind_addr: String,          // e.g. "0.0.0.0"
    pub bind_port: u16,             // Port: 5000
    pub multicast_addr: String,     // e.g. "239.192.0.1" = IP + Port 
    pub multicast_port: u16,        // Port: 5000
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
    pub fn new(bind_addr: &str, bind_port: u16, multicast_addr: &str, multicast_port: u16) -> Self {
        
        Self {
            config: UdpConfig {
                bind_addr: bind_addr.to_string(),
                bind_port: bind_port,
                multicast_addr: multicast_addr.to_string(),
                multicast_port: multicast_port,
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
                .split(':')
                .next()
                .ok_or_else(|| anyhow::anyhow!("Missing multicast IP"))?
                .parse()?;

            let iface_str = self
                .config
                .iface_addr
                .as_deref() // converts Option<String> to Option<&str>
                .ok_or_else(|| anyhow::anyhow!("Missing iface IP"))?;

            let iface_ip = iface_str
                .split(':')
                .next()
                .ok_or_else(|| anyhow::anyhow!("iface_addr has invalid format"))?
                .parse::<Ipv4Addr>()?;

            socket
                .join_multicast_v4(&multi, &iface_ip)
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

    use anyhow::Result;  // to allow test_udp_source_receives_data()
    use crate::udp_source;
        #[test]
    //fn test_udp_source_receives_data() -> Result<(), Box<dyn std::error::Error>> {
    fn test_udp_source_receives_data() -> Result<()> {
        let sender = UdpSocket::bind("127.0.0.1:0")?;
        sender.send_to(&[0xAB], "127.0.0.1:6000")?;
        // allow some time for sender to ramp up
        thread::sleep(Duration::from_millis(5)); // Wait for socket to receive

        let (mut src, rx) = UdpSourceBuilder::<u8>::new("127.0.0.1", 6000, 
        "239.0.0.1", 6000)
            .iface_addr("127.0.0.1")
            .reuse_addr(true)
            .build()?;

        src.work().unwrap();

        let (reader, _tags) = rx.read_buf()?; // You were here
        assert_eq!(reader[0], 0xAB);

        Ok(())
    }
#[test]
fn test_udp_source_receives_incrementing_bytes() -> anyhow::Result<()> {
    use std::{net::UdpSocket, thread, time::Duration};
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;

    // Port shared by both sender and receiver
    const TEST_PORT: u16 = 6000;

    // Shared counter for incrementing byte values
    let counter = Arc::new(AtomicU8::new(0));
    let counter_clone = counter.clone();

    // Spawn continuous sender
    thread::spawn(move || {
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        loop {
            let value = counter_clone.fetch_add(1, Ordering::Relaxed);
            let _ = sender.send_to(&[value], &format!("127.0.0.1:{TEST_PORT}"));
            thread::sleep(Duration::from_millis(10));
        }
    });

    // Give sender a moment to start
    thread::sleep(Duration::from_secs(1));

    // Create the UDP source
    let (mut src, rx) = UdpSourceBuilder::<u8>::new("0.0.0.0", TEST_PORT, "239.0.0.1", TEST_PORT)
        .iface_addr("127.0.0.1")
        .reuse_addr(true)
        .build()?;

    // Try up to N rounds to get 2 valid samples
    // let mut previous = None;
    //let mut previous: std::option::Option = None;

    for _ in 0..20 {
        src.work()?;
        let (reader, _) = rx.read_buf()?;
        if reader.len() >= 2 {
            let a = reader[0];
            let b = reader[1];
            assert_eq!(b.wrapping_sub(a), 1, "Values: a = {a}, b = {b}");
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }

    Err(anyhow::anyhow!("Failed to receive 2 sequential values"))
}


}

