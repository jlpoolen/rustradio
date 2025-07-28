/*
cd /usr/local/src/rustradio 
clear; date; cargo run  --example airspy_udp_decode --features audio,logging


Sample run:
have rpi5 tmux RTL-SDR_UPD running:
    date; rtl_sdr -d 0 -f 124550000 -s 288000 -g 30 - | socat - UDP-DATAGRAM:239.192.0.3:5003,range=239.0.0.0/8 &

    Confirm something to read: 
    (Note: you have to use the actual IP of the machine you are running this command from)
    timeout 1 socat -u UDP-RECV:5003,reuseaddr,reuseport,ip-add-membership=239.192.0.3:192.168.1.103 - | hexdump -C

Then:
    cargo run --example udp_decode --features "audio" -- -m 239.192.0.3 -p 50003 --samp-rate 288000 --format u8
or:
     cargo build --release --example udp_decode --features="audio"
then:
    cd /usr/local/src/rustradio
    target/release/examples/udp_decode -m 239.192.0.3 -p 50003 --samp-rate 288000 --format u8 --volume 8
    
*/
use anyhow::Result;
use clap::{Arg, ArgAction, Command, Parser, ValueEnum};
//old use log::warn;
use log::{warn};

use std::borrow::Cow;
//use std::net::IpAddr;
use std::path::{ PathBuf};

use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;

use anyhow::anyhow;

//use tracing::{debug, error, trace};

use num_complex::Complex;
use rustradio::blocks::*;
use rustradio::graph::GraphRunner;
use rustradio::mtgraph::MTGraph;
use rustradio::parse_verbosity;
use rustradio::udp_source::UdpSourceBuilder;
use rustradio::{ComplexU8, Float, blockchain};

// additional for debugging ChatGTP suggested debug statement
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use std::time::{SystemTime, UNIX_EPOCH};
use chrono::Local;


fn now_string_epoch() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    format!(
        "{}.{:06}",
        now.as_secs(),
        now.subsec_micros() // for microsecond precision (change to `nanos()` for nanoseconds)
    )
}

fn now_string() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

#[derive(ValueEnum, Clone, Debug)]
enum Format {
    I16,
    U8,
}


#[derive(clap::Parser, Debug)]
#[command(version, about)]
struct Opt {
    /// Format of samples (i16 for AirSpy, u8 for RTL-SDR)
    #[arg(long, value_enum)]
    format: Format,

    /// Input file in airspy format (I/Q s16)
    #[arg(short)]
    input: Option<String>,

    #[arg(short, value_parser=parse_verbosity, default_value="info")]
    verbose: usize,

    #[arg(long = "volume", default_value = "0.1")]
    volume: Float,
    
    #[arg(short, long)]
    multicast_addr: String,

    #[arg(short, long, required_unless_present_any=["port_local", "port_multicast"])]
    port: Option<u16>,

    #[arg(long, requires = "port_multicast")]
    port_local: Option<u16>,

    #[arg(long, requires = "port_local")]
    port_multicast: Option<u16>,

    #[arg(long)]
    iface_addr: Option<String>,

    #[arg(long, default_value = "true")]
    reuse: bool,

    #[arg(long, default_value = "2500000")]
    samp_rate: u32,

    #[arg(long, default_value = "48000")]
    audio_rate: usize,

    #[arg(long)]
    filename: Option<String>,
}

pub fn main() -> Result<()> {
    //tracing_subscriber::fmt::init();

    let opt = Opt::parse();

    // Use multicast_addr directly
    let multicast_addr = opt.multicast_addr.clone();

    // Handle port logic
    let port_local: u16;
    let port_multicast: u16;

    if let Some(p) = opt.port {
        port_local = p;
        port_multicast = p;
    } else {
        port_local = opt.port_local.ok_or_else(|| {
            anyhow::anyhow!("Must provide --port or both --port_local and --port_multicast")
        })?;
        port_multicast = opt.port_multicast.ok_or_else(|| {
            anyhow::anyhow!("Must provide both --port_local and --port_multicast")
        })?;
    }

    // Determine interface address
    let iface_addr = if let Some(addr) = &opt.iface_addr {
        addr.clone()
    } else {
        local_ip_address::local_ip()
            .map_err(|_| anyhow::anyhow!("Failed to auto-detect interface IP"))?
            .to_string()
    };

    // Use reuse as boolean already parsed by Clap
    let reuse_addr = opt.reuse;

    // Sample and audio rates are already properly typed
    //let samp_rate = opt.samp_rate;
    //let audio_rate = opt.audio_rate;

    // Audio output flag and filename
    let (output_audio, file_name): (bool, Option<String>) = match &opt.filename {
        Some(f) => {
           if let Some(fname) = &opt.filename {
                let output_path = Path::new(fname);
                let metadata = fs::metadata(output_path)
                    .map_err(|_| anyhow!("Output path does not exist: {}", fname))?;

                let file_type = metadata.file_type();
                if !file_type.is_file() && !file_type.is_fifo() {
                    return Err(anyhow!("Output path must be a regular file or FIFO: {}", fname));
                }
            }
            (false, opt.filename)
        }
        None => (true, None),
    };



    let verbosity = if cfg!(debug_assertions) {
        opt.verbose
    } else {
        0 // suppress info/debug/trace in release builds
    };
    stderrlog::new()
        .module(module_path!())
        .module("rustradio")
        .quiet(false)
        .verbosity(verbosity)
        .timestamp(stderrlog::Timestamp::Second)
        .init()?;

    let mut g = MTGraph::new();
    //let samp_rate = 2_500_000f32;
    let samp_rate = opt.samp_rate;  // x6 of target audio rate of 48k
    //let samp_rate = 144_000f32;  // x3  playback is slowed down
    let audio_rate = opt.audio_rate;

        // Display for verification
    println!("udp_decode");
    println!("Using fixed multicast address:port: 239.192.0.3:5003 -- TRANSMISSION MUST MATCH THIS");
    println!("Using interface: {}", iface_addr);
    println!(
        "Multicast group: {}, Ports: local={}, multicast={}",
        multicast_addr, port_local, port_multicast
    );
    println!("Sample rate: {}, Audio rate: {}", samp_rate, audio_rate);
    println!("Reuse addr: {}", reuse_addr);

    let prev = blockchain![
        g,
        prev,
        UdpSourceBuilder::<ComplexU8>::new(
            "0.0.0.0",     // bind address
            5003, //5000,          // local port
            "239.192.0.3", //"239.192.0.1", // multicast group
            5003 //5000           // multicast port
        )
        .iface_addr(&iface_addr) // ← your NIC's IP
        .reuse_addr(true) 
        .build()?,

// Map::new(prev, "Raw ComplexU8 monitor", |s: ComplexU8, tags| {
//     static COUNT: AtomicUsize = AtomicUsize::new(0);
//     let c = COUNT.fetch_add(1, Ordering::Relaxed);
//     if c % 288000 == 0 {
//         println!("[{}] [UDP INPUT] Sample count: {}", now_string(), c);
//     }
//     (s, Cow::Borrowed(tags))
// }),



        Map::new(prev, "ComplexU8 to Complex<f32>", |x: ComplexU8, tags| {
            (
                Complex::new(x.re as f32 - 128.0, x.im as f32 - 128.0)/128.0,
                Cow::Borrowed(tags),
            )
        }),
        FftFilter::new(
            prev,
            rustradio::fir::low_pass_complex(
                samp_rate as f32,
                12_500.0,
                10_000.0,
                &rustradio::window::WindowType::Hamming,
            )
        ),
        Map::keep_tags(prev, "am decode", |v| v.norm()),
        FftFilterFloat::new(
            prev,
            &rustradio::fir::low_pass(
                samp_rate as f32,
                audio_rate as Float,
                500.0,  // was 500
                &rustradio::window::WindowType::Hamming,
            )
        ),

// Map::new(prev, "Sample Monitor", |s: f32, tags| {
//     static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
//     let c = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
//     if c % 48000 == 0 {
//         println!("[Before] Sample count: {}", c);
//     }
//     (s, Cow::Borrowed(tags))
// }),

        RationalResampler::builder()
            .deci(samp_rate as usize)
            .interp(audio_rate)
            .build(prev)?,

// Map::new(prev, "Sample Monitor", |s: f32, tags| {
//     static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
//     let c = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
//     if c % 48000 == 0 {
//         println!("[After] Sample count: {}", c);
//     }
//     (s, Cow::Borrowed(tags))
// }),

        MultiplyConst::new(prev, opt.volume),
        // ChatGpt suggestion to convert to i16 PCM
        // Map::new(prev, "f32 to i16", |x: f32, tags| {
        //     (
        //         (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16,
        //         Cow::Borrowed(tags)
        //     )
        //  }),
// ChatGPT 7/6 10:43 AM suggestion, removed at 11:00 AM, this simply just moved the problem down
// the line, 2x playback continue to be problem and below requires remming out audio.  This is
// not helpful and, in fact, limits my ability to play audio directly.
// Map::new(prev, "f32 to i16", |x: f32, tags| {
//     (
//         (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16,
//         Cow::Borrowed(tags)
//     )
// })
    ];

    if output_audio {
        //g.add(Box::new(AudioSink::new(prev, audio_rate as u64)?));  <-- original, below has better error handling
        let audio_sink = AudioSink::new(prev, audio_rate as u64).expect("Failed to create AudioSink");
        g.add(Box::new(audio_sink));
    } else {
        let file_path = PathBuf::from(file_name.as_ref().unwrap());
        g.add(Box::new(FileSink::new(
            prev,
            file_path,
            rustradio::file_sink::Mode::Overwrite,
            //rustradio::file_sink::Mode::Append,
        )?));
    }

    let cancel = g.cancel_token();
    ctrlc::set_handler(move || {
        warn!("Got Ctrl-C");
        eprintln!("\n");
        cancel.cancel();
    })
    .expect("failed to set Ctrl-C handler");
    eprintln!("Running loop");
    g.run()?;
    eprintln!("{}", g.generate_stats().unwrap());
    Ok(())
}
