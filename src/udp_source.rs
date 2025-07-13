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

use socket2::{Socket, Domain, Type, Protocol, SockRef, SockAddr};
use std::net::SocketAddr;

//use std::net::{};
//use tracing::{info, debug, warn, error};

//use rustradio_macros::rustradio;
use crate::block::{Block, BlockRet, BlockEOF, BlockName};
//use crate::circular_buffer::BufferReader;
use crate::stream::{ReadStream, WriteStream, Tag, TagValue}; 
use crate::{Result, Sample};
use tracing::{info};


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
        info!("🧩 UdpSourceBuilder self at {:p}", &self as *const _);
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

        if self.config.reuse.unwrap_or(false) {
            socket
                .set_reuse_address(true)
                .context("Failed to set SO_REUSEADDR")?;
            #[cfg(target_os = "linux")]
            socket
                .set_reuse_port(true)
                .context("Failed to set SO_REUSEPORT")?;
            info!("socket reuse address and port set to true.");
        }
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

            // Merge bind_addr and bind_port into a full socket address string
            let bind_addr_merged_str = format!("{}:{}", self.config.bind_addr, self.config.bind_port);

            // Parse the merged string into a SocketAddr
            let bind_addr_merged: SocketAddr = bind_addr_merged_str
                .parse()
                .with_context(|| format!("Failed to merge bind_addr '{}' with port {}", self.config.bind_addr, self.config.bind_port))?;

            // Log the address we are about to bind
            info!("Binding to {}", bind_addr_merged);

            // Convert std::net::SocketAddr into socket2::SockAddr and bind
            socket
                .bind(&SockAddr::from(bind_addr_merged))
                .context("Failed to bind socket")?;


            socket
                .join_multicast_v4(&multi, &iface_ip)
                .context("Failed to join multicast group")?;

            info!(
                "Joined multicast group: {} on iface: {} (port: {}), multicast mode: {}",
                multi,
                iface_ip,
                self.config.multicast_port,
                !self.config.multicast_addr.is_empty()
            );

        }
        


        socket
            //.set_nonblocking()
            .set_nonblocking(true)
            .context("Failed to set non-blocking(true) mode")?;
            //.set_nonblocking(false)?;
        info!("Temporary: activating: socket.set_nonblocking(true)");
        //info!("Temporary: activating: socket.set_nonblocking(false)");
        //info!("Temporary: suspending socket nonblocking to 'true' or 'false'");

        let sockref = SockRef::from(&socket);
        info!("Socket non-blocking status: {:?}", sockref.nonblocking());

        // let (tx, rx) = crate::stream::new_stream();
        // info!("💡 rx stream address = {:p}", &rx as *const _);
        // info!("💡 tx stream address = {:p}", &tx as *const _);
        // let udp_socket: std::net::UdpSocket = socket.into();
        // let udp_source = UdpSource {
        //     socket: udp_socket,
        //     buffer: [0u8; 4096],
        //     dst: tx,  
        // };

        // Ok((udp_source, rx))

        let (tx, rx) = crate::stream::new_stream(); // tx = WriteStream, rx = ReadStream
        info!("[UdpSourceBuilder ] 🟢 tx stream address = {:p}", &tx as *const _);
        info!("[UdpSourceBuilder ] 🟢 rx stream address = {:p}", &rx as *const _);

        let udp_socket: std::net::UdpSocket = socket.into();

        let udp_source = UdpSource {
            socket: udp_socket,
            buffer: [0u8; 4096],
            dst: tx, // ✅ UDP writes into the WriteStream
        };

        Ok((udp_source, rx)) // ✅ Return the ReadStream to the test

        //Ok((udp_source, rx))  // errors with: ^^ expected `ReadStream<T>`, found `WriteStream<T>` 
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
        info!("[UdpSource Block.work] Work commencing.");
        //let (input_buffer, input_tags) = self.src.read_buf();
        let mut output_stream = self.dst.write_buf()?;
        let empty_tags: &[Tag] = &[];
            match self.socket.recv(&mut self.buffer) {
                Ok(n) => {
                    //println!("📐 T::size() = {}", T::size());               // additional
                    //println!("📐 Received n = {}", n);                      // additional
                    println!("[UdpSource Block.work] 📐 n % T::size() = {}", n % T::size());       // additional
                    let chunked = self.buffer[..n].chunks_exact(T::size()); 
                    let remainder = chunked.remainder();
                    info!("[UdpSource Block.work] Received {} bytes", n);
                    info!("[UdpSource Block.work] chunked.size: {}", chunked.clone().count());
                    let mut count = 0; 
                    
                    if let Ok(writer) = self.dst.write_buf() {
                        for chunk in chunked {
                            //info!("[UdpSource Block.work] 🔍 Parsing chunk: {:02X?}", chunk);      // additional
                            info!(
                                "[UdpSource Block.work] 🔍 Parsing chunk: [{}] [{}]",
                                chunk.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" "),
                                chunk.iter()
                                    .map(|&b| if (0x20..=0x7E).contains(&b) { b as char } else { '.' })
                                    .collect::<String>()
                            );
                            match T::parse(chunk) {
                                Ok(sample) => {
                                    info!("✅ Parsed sample: {:?}", sample); // additional
                                    // ORIGINAL if let Ok(mut writer) = self.dst.write_buf() {
                                    // ORIGINAL     writer.fill_from_slice(&[sample]);  // only if you have a full slice
                                    // ORIGINAL }
                                    match T::parse(chunk) {
                                        Ok(writer) => {
                                            info!("✍️ Writing parsed sample to ring buffer: {:?}", sample);
                                            info!("[UdpSource Block.work] ✍ writer acquired address from fn work()'s dst {:p}", &self.dst);  // aditional +
                                            let output_slice = output_stream.slice();

                                            let max_output_samples = output_slice.len();
                                            info!("max_output_samples: {}", max_output_samples);
                                            let mydata = sample;
                                            let out_len = std::mem::size_of::<T>();
                                            
                                            output_slice[..out_len].copy_from_slice(&[mydata]);
                                            //output_stream.produce(out_len);
                                            //OLD: writer.clone_from(&[sample]); 
                                            
                                        }
                                        Err(e) => {
                                            println!("[UdpSource Block.work] ❌ Could not get write buffer: {:?}", e);
                                        }
                                    }
                                    count += 1;
                                }
                                Err(e) => {
                                    warn!("Failed to parse sample: {:?}", e);
                                    println!("[UdpSource Block.work] ❌ Failed to parse sample: {:?}", e); // additional
                                }
                            }
                        }
                        if count > 0 {
                            
                            writer.produce(count,empty_tags); // ✅ REQUIRED!
                        }
                    } else {
                        println!("[UdpSource Block.work] ❌ Could not get write buffer");
                    }
                    info!("[UdpSource Block.work] Parsed {} valid samples", count);
                    if !remainder.is_empty() {
                        warn!(
                            "[UdpSource Block.work] Discarding {} leftover bytes (incomplete sample)",
                            remainder.len()
                        );
                    }

                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No data available (non-blocking)
                    warn!("[UdpSource Block.work] No datagram available yet");
                }
                //Err(e) => {
                //    return Err(anyhow::anyhow!("recv failed: {}", e));
                //}
                Err(e) => {
                    //error!("UDP recv failed: {}", e);
                    warn!("[UdpSource Block.work] UDP recv failed: {}", e);
                }
                //Err(_) => todo!()
                //Err(_) => {
                //    warn!("reached Err(_) todo!");
                //}

                //Err(e) => {
                //    return Err(anyhow::anyhow!("UDP recv failed: {}", e));
                //}
            }
            info!("[UdpSource Block.work] work at Ok.");
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

#[test]
    /* To run this test:

    date;RUST_LOG=info  RUST_BACKTRACE=1  cargo test test_1_udp_source_receives_data --features logging --lib |nl

    This test fails, probably because sending a single byte starves any buffers?

     */
fn test_1_udp_source_receives_data() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    //let sender = UdpSocket::bind("127.0.0.1:0")?;
    let sender = UdpSocket::bind("192.168.1.2:0")
    .unwrap_or_else(|e| panic!("❌ Failed to bind: {e}"));
   
    // send a single byte  (may not work as we might be starving buffers)
    sender.send_to(&[0xAB], "239.0.0.1:6000")
    .unwrap_or_else(|e| panic!("❌ Failed to send: {e}"));
    // allow some time for sender to ramp up
    thread::sleep(Duration::from_millis(5)); // Wait for socket to receive

    let (mut src, rx) = UdpSourceBuilder::<u8>::new(
        "127.0.0.1", 6000, 
        "239.0.0.1", 6000)
        .iface_addr("192.168.1.2")
        .reuse_addr(true)
        .build()
        .unwrap_or_else(|e| panic!("❌ Failed to build UdpSourceBuilder: {e}"));

    info!("Befpre calling fn work().");
    src.work().unwrap();
    info!("After calling fn work().");

    let (reader, _tags) = rx.read_buf()?; // You were here
    // Convert to slice so we can call `.chunks()`
    let data = reader.slice();

    // Print only ASCII characters like `hexdump -C` right column
    for chunk in data.chunks(16) {
        for &byte in chunk {
            let printable = if (0x20..=0x7e).contains(&byte) {
                byte as char
            } else {
                '.'
            };
            print!("{}", printable);
        }
        println!();
    }

    // Or full `hexdump -C` style (hex + ascii side-by-side)
    for (i, chunk) in data.chunks(16).enumerate() {
        print!("{:08x}  ", i * 16);
        for byte in chunk.iter() {
            print!("{:02x} ", byte);
        }
        for _ in 0..(16 - chunk.len()) {
            print!("   ");
        }
        print!(" |");
        for &byte in chunk {
            let printable = if (0x20..=0x7e).contains(&byte) {
                byte as char
            } else {
                '.'
            };
            print!("{}", printable);
        }
        println!("|");
    }

    assert_eq!(reader[0], 0xAB);

    Ok(())
}

#[test]
fn test_2_udp_source_receives_incrementing_bytes() -> anyhow::Result<()> {
/*
    Command to run this test 2:

      date; timeout 60 cargo test test_2_udp_source_receives_incrementing_bytes --features logging --lib |nl

    In a separate console, to confirm broadcaster use this preferred method:

       timeout 3 socat -u UDP-RECV:6000,reuseaddr,reuseport,ip-add-membership=239.0.0.1:127.0.0.1 - | hexdump -C

    Alternative methods of testing include:

        date; nc -lu -p 6000
    
    or

        date; timeout 0.04 sudo tcpdump -n -i lo udp port 6000 |nl

*/
    use std::{net::UdpSocket, thread, time::Duration};
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;

    let _ = tracing_subscriber::fmt::try_init();

    // Port shared by both sender and receiver
    const TEST_PORT: u16 = 6000;

    // Shared counter for incrementing byte values
    let counter = Arc::new(AtomicU8::new(0));
    let counter_clone = counter.clone();

    // Spawn a continuous sender
    if false {
        // 1 byte transmission     
        thread::spawn(move || {
            let sender = UdpSocket::bind("127.0.0.1:0")
            .unwrap_or_else(|e| panic!("❌ Failed to bind: {e}"));
            loop {
                let value = counter_clone.fetch_add(1, Ordering::Relaxed);
                //let _ = sender.send_to(&[value], &format!("239.0.0.1:{TEST_PORT}"))
                //.unwrap_or_else(|e| panic!("❌ Failed to send: {e}"));
                //thread::sleep(Duration::from_millis(10));
                // an alternative to see if we can read ASCII
                let payload = b"The quick brown fox jumps over the lazy dog 1234567890!";
                sender.send_to(payload, &format!("239.0.0.1:{TEST_PORT}"))
                .unwrap_or_else(|e| panic!("❌ Failed to send: {e}"));
            }
        });
    } else {
        // 2 bytes
        thread::spawn(move || {
            let sender = UdpSocket::bind("127.0.0.1:0")
                .unwrap_or_else(|e| panic!("❌ Failed to bind: {e}"));

            loop {
                let value = counter_clone.fetch_add(1, Ordering::Relaxed);
                let bytes = (value as u16).to_le_bytes(); // Convert to 2 bytes (little endian)
                sender
                    .send_to(&bytes, &format!("239.0.0.1:{TEST_PORT}"))
                    .unwrap_or_else(|e| panic!("❌ Failed to send: {e}"));
                // thread::sleep(Duration::from_millis(10));
            }
        });
    }




    // Give sender a moment to start
    //thread::sleep(Duration::from_secs(1));
    //thread::sleep(Duration::from_millis(100));

    // Create the UDP source
    let (mut src, rx) = UdpSourceBuilder::<u8>::new("0.0.0.0", 
        TEST_PORT, "239.0.0.1", TEST_PORT)
        .iface_addr("192.168.1.2")
        .reuse_addr(true)
        .build()
        .unwrap_or_else(|e| panic!("❌ Failed to call build() on UdpSourceBuilder: {e}"));
    info!("Created UdpSource.");

    // Try up to N rounds to get 2 valid samples
    // let mut previous = None;
    //let mut previous: std::option::Option = None;
    for _ in 0..20 {
    //loop {
        src.work()
        .unwrap_or_else(|e| panic!("❌ Failed to call work(): {e}"));
        info!("[Test 2] After calling src.work()");
        let (reader, _) = rx.read_buf()
        .unwrap_or_else(|e| panic!("❌ Failed to read_buf: {e}"));

        //info!("[Test 2] reader.total_size() {}", reader.parent.total_size());
        //if rx.total_size() < 1 {
            // the buffer does not have anything in it
            // what can we do until total_size() becomes > 0?
            //return Ok(());
        //}

        if reader.is_empty() {
             info!("[Test 2] reader is empty.");
             //return Ok(());
             
        } else {
            info!("[Test 2] reader is NOT empty.");
        }
        info!("reader address (rx): {:p}", &rx as *const _);
        info!("[Test 2] After creating reader, length = {}", reader.len());
        if reader.len() >= 2 {
        //if true { // force entry into this clause
            info!("[Test 2] In reader >=1 clause.");
            let data: &[u8] = reader.slice();
            info!("[Test 2] data: {:?}", data);
            //let a = reader[0];
            //let b = reader[0];  // was 1, changing to 0 since length = 1
            //assert_eq!(b.wrapping_sub(a), 1, "Values: a = {a}, b = {b}");
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }

    Err(anyhow::anyhow!("Failed to receive 2 sequential values"))
}

/*
Command to run this test:

      date; timeout 60 cargo test test_3_udp_source_receives_data_subscribe_first --features logging --lib |nl

*/
#[test]
fn test_3_udp_source_receives_data_subscribe_first() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    use std::{net::UdpSocket, thread, time::Duration};

    // --- CONFIGURATION PARAMETERS ---
    let interface_name = "enp5s0";          // Confirmed NIC
    let interface_ip = "192.168.1.2";       // IP bound to enp5s0
    let bind_addr = "0.0.0.0";  
    let bind_port = 6000;              // Bind to all interfaces
    let multicast_addr = "239.0.0.1";
    let multicast_port = 6000;
    //let test_payload = [0xAB];
    let test_payload: Vec<u8> = (0..=255).cycle().take(4096).collect();
    let max_attempts = 20;
    let delay_ms = 50;

    // --- DIAGNOSTIC OUTPUT ---
    println!("\n--- Test Parameters ---");
    println!("Interface Name  : {}", interface_name);
    println!("Interface IP    : {}", interface_ip);
    println!("Bind IP         : {}", bind_addr);
    println!("Bind Port       : {}", bind_port);
    println!("Multicast Addr  : {}", multicast_addr);
    println!("Multicast Port  : {}", multicast_port);
    println!("test_payload is 4096 characters.");
    //println!("Payload         : {:02X?}", test_payload);
    println!("Max Attempts    : {}", max_attempts);
    println!("Delay Per Try   : {}ms", delay_ms);
    println!("------------------------\n");

    // --- STEP 1: Start multicast receiver ---
    println!("Setting up UdpSourceBuilder...");

    let builder = UdpSourceBuilder::<u8>::new(
        bind_addr, bind_port, 
        multicast_addr, multicast_port)
        .iface_addr(interface_ip)
        .reuse_addr(true);

    println!("🧪 UdpSourceBuilder instance at {:p}", &builder as *const _);

    let (mut src, rx) = builder.build()?;

    // START Proposed replacement for above
    // let builder = UdpSourceBuilder::<u8>::new(bind_ip, port, multicast_ip, port)
    //     .iface_addr(interface_ip)
    //     .reuse_addr(true);

    // println!("🧪 UdpSourceBuilder instance at {:p}", &builder as *const _);

    // let (mut src, rx) = builder.build()?;
    // END proposed replacement

    println!("🧪 rx address from test = {:p}", &rx as *const _);

    println!("test's socket dst ptr  = {:p}", &src.dst);
    println!("test's socket rx ptr   = {:p}  --- this should match fn work()'s dst ptr which was being written to", &rx);
    println!("UdpSourceBuilder initialized. Waiting 200ms for multicast join...");
    thread::sleep(Duration::from_millis(200));

    // --- STEP 2: Send multicast packet ---
    let sender = UdpSocket::bind((interface_ip, 0))?;
    let dest_addr = format!("{}:{}", multicast_addr, multicast_port);
    let sent = sender.send_to(&test_payload, &dest_addr)?;
    println!("[UDP Broadcaster] Sent {} byte(s) to {}", sent, dest_addr);
    // The broadcast can be confirmed in a separate console where socat had been started prior to this program.

    // --- STEP 3: Retry read loop ---
    let mut received = false;
    for attempt in 0..max_attempts {
        thread::sleep(Duration::from_millis(delay_ms));
        println!("Attempt {}: calling src.work()", attempt + 1);
        src.work().unwrap();

        // ORIGINAL let (reader, _tags) = rx.read_buf()?;
        let mut waited = 0;
        let mut reader;
        loop {
            let (r, _tags) = rx.read_buf()?;
            reader = r;
            if !reader.is_empty() {
                break;
            }
            if waited >= 100 {
                println!("⚠️ Timeout waiting for buffer to be readable.");
                break;
            }
            waited += 1;
            thread::sleep(Duration::from_millis(10));
        }
        let data = reader.slice();

        println!("[Test Reader]  Received {} byte(s)", data.len());
        if !data.is_empty() {
            println!("[Test Reader]   Received data (ASCII):");
            // for chunk in data.chunks(16) {
            //     for &b in chunk {
            //         print!("{}", if (0x20..=0x7E).contains(&b) { b as char } else { '.' });
            //     }
            //     println!();
            // }

            for (i, chunk) in data.chunks(16).enumerate() {
                print!("{:08x}  ", i * 16);
                for byte in chunk.iter() {
                    print!("{:02x} ", byte);
                }
                for _ in 0..(16 - chunk.len()) {
                    print!("   ");
                }
                print!(" |");
                for &byte in chunk {
                    let printable = if (0x20..=0x7e).contains(&byte) {
                        byte as char
                    } else {
                        '.'
                    };
                    print!("{}", printable);
                }
                println!("|");
            }
            
            assert_eq!(data[0], test_payload[0], "[Test Reader] First byte does not match test payload.");
            received = true;
            break;
        }
    }

    assert!(received, "[Test Reader] Did not receive expected UDP packet after {} attempts.", max_attempts);
    Ok(())
}



/*  I have a Raspberry Pi c program, ./airspy_rx_minimalm, running broadcasting IQs live

    Sending IQ stream to 239.192.0.1:5000

Confirming there's something to read:

    timeout 3 socat -u UDP-RECV:5000,reuseaddr,reuseport,ip-add-membership=239.192.0.1:127.0.0.1 - | hexdump -C
    00000000  5b ff 36 00 0c 00 f4 ff  7e 00 99 ff 21 00 ba ff  |[.6.....~...!...|
    00000010  02 00 54 00 3e 00 dd ff  02 00 f1 ff 91 ff ff ff  |..T.>...........|
    00000020  51 ff 2b 00 96 ff 08 00  1e 00 00 00 de ff 1f 00  |Q.+.............|

*/
//#[test]
// fn test_outside_udp_server() -> anyhow::Result<()> {
//     use std::thread;
//     use std::time::Duration;

//     let (mut src, rx) = UdpSourceBuilder::<i16>::new("0.0.0.0", 5000, "239.192.0.1", 5000)
//         .iface_addr("192.168.1.2")
//         .reuse_addr(true)
//         .build()?;

//     tracing::info!("Waiting 1 second before starting receive loop...");
//     thread::sleep(Duration::from_secs(1));

//     let mut received_count = 0;

//     for i in 0..200 {
//         tracing::info!("work cycle {}", i);
//         src.work()?;

//         if let Ok((reader, _)) = rx.read_buf() {
//             let slice = reader.slice();
//             tracing::info!("Received {} bytes: {:?}", slice.len(), &slice[..slice.len().min(10)]);
//             received_count += slice.len();
//         }

//         if received_count >= 5 {
//             tracing::info!("Received {} total bytes, exiting early", received_count);
//             return Ok(());
//         }

//         thread::sleep(Duration::from_millis(100));
//     }

//     Err(anyhow::anyhow!("Did not receive enough datagrams from external source"))
// }

}