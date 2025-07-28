fn main() -> anyhow::Result<()> {
    use rustradio::udp_source::UdpSourceBuilder;
   use rustradio::stream::ReadStream;
   use rustradio::block::Block;

    use std::{thread, time::Duration};
    //use tracing_subscriber::fmt::init;

   // init(); // Enable info logging

    let test_name = "UDP u8 Listener";

    // === CONFIGURATION ===
    let interface_ip = "192.168.1.2";
    let bind_addr = "0.0.0.0";
    let bind_port = 5003;
    let multicast_addr = "239.192.0.3";
    let multicast_port = 5003;

    println!("\n--- {test_name} ---");
    println!("Interface IP    : {interface_ip}");
    println!("Bind Addr       : {bind_addr}");
    println!("Bind Port       : {bind_port}");
    println!("Multicast Addr  : {multicast_addr}");
    println!("Multicast Port  : {multicast_port}");

    let (mut src, rx): (_, ReadStream<u8>) = UdpSourceBuilder::<u8>::new(
        bind_addr, bind_port, multicast_addr, multicast_port,
    )
    .iface_addr(interface_ip)
    .reuse_addr(true)
    .reuse_port(true)
    .build()
    .unwrap();

    println!("Waiting 1 second for warm-up...");
    thread::sleep(Duration::from_secs(1));

    let mut received_total = 0;
    for i in 0..60 {
        println!("[{test_name}] work cycle {i}");
        src.work()?;

        if let Ok((reader, _)) = rx.read_buf() {
            let slice = reader.slice();
            println!(
                "Received {} bytes: {:?}",
                slice.len(),
                &slice[..slice.len().min(16)]
            );
            received_total += slice.len();
        }

        thread::sleep(Duration::from_millis(250));
    }

    println!("[{test_name}] Total bytes received: {received_total}");
    Ok(())
}
