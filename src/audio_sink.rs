use crate::Result;
use cpal::Sample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::{debug, error, info, trace};

use crate::block::{Block, BlockRet};
use crate::graph::CancellationToken;
use crate::stream::ReadStream;
use crate::{Error, Float};

use std::sync::mpsc::{SyncSender, sync_channel};

struct CpalOutput {
    device: cpal::Device,
    config: cpal::StreamConfig,
}

crate::error_from!(
    "audio",
    cpal::PlayStreamError,
    cpal::BuildStreamError,
    cpal::DevicesError,
    cpal::DeviceNameError,
    cpal::SupportedStreamConfigsError,
    cpal::DefaultStreamConfigError,
);

impl CpalOutput {
    fn new(sample_rate: u32, device_name: Option<&str>) -> Result<Self> {
        for host in cpal::platform::ALL_HOSTS {
            debug!("Audio sink host: {host:?}, name: {}", host.name());
        }
        let host = cpal::default_host();
        // let host = cpal::host_from_id(cpal::platform::ALL_HOSTS[0])?;
        debug!("Audio sink chose default host {}", host.id().name());
        if false {
            // Printing device names spews a bunch of ALSA errors to stderr.
            // https://github.com/RustAudio/cpal/issues/384
            for dev in host.devices()? {
                debug!("Audio sink device: {:?}", dev.name()?);
            }
        }
        let device = if let Some(dn) = device_name {
            host.output_devices()?
                .find(|d| d.name().unwrap_or_default().contains(dn))
                .ok_or(Error::msg("audio sink: failed to find output device {dn}"))?
        } else {
            host.default_output_device()
                .ok_or(Error::msg("audio sink: failed to find output device"))?
        };
        info!("Audio sink output device: {}", device.name()?);

        trace!("Audio sink supported output configs:");
        for conf in device.supported_output_configs()? {
            trace!("  {conf:?}");
        }

        let config = device.default_output_config()?;
        debug!("Audio sink using default output config {config:?}");

        let mut config: cpal::StreamConfig = config.into();

        config.sample_rate = cpal::SampleRate(sample_rate);
        config.channels = 1;

        Ok(Self { device, config })
    }

    fn start(&self) -> Result<(SyncSender<f32>, cpal::Stream)> {
        let (sender, receiver) = sync_channel::<f32>(self.config.sample_rate.0 as usize * 3); // 3 seconds buffer

        let channels = self.config.channels as usize;
        let err_fn = |err| error!("an error occurred on stream: {err}");

        let device = self.device.clone();
        let config = self.config.clone();

        info!("Starting audio output stream {config:?}");
        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    match receiver.recv() {
                        Err(e) => {
                            info!("Failed to read audio samples: {e:?}");
                        }
                        Ok(v) => {
                            let value = f32::from_sample(v);
                            for sample in frame.iter_mut() {
                                *sample = value;
                            }
                        }
                    }
                }
            },
            err_fn,
            None,
        )?;
        stream.play()?;
        Ok((sender, stream))
    }
}

/// Audio sink builder.
#[derive(Default)]
pub struct AudioSinkBuilder {
    dev: Option<String>,
}

impl AudioSinkBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }
    /// Build the AudioSink.
    pub fn build(self, prev: ReadStream<Float>, sample_rate: u64) -> Result<AudioSink> {
        AudioSink::new_opts(prev, sample_rate, self.dev.as_deref())
    }
}

/// Audio sink. In other words: playback to speakers.
#[derive(rustradio_macros::Block)]
#[rustradio(crate)]
pub struct AudioSink {
    #[rustradio(in)]
    src: ReadStream<Float>,
    sender: Option<SyncSender<f32>>,

    // The cpal::Stream is not Send, but needs to be kept alive for the duration
    // of this block's lifetime. So we spawn a thread just to own that stream.
    //
    // This way we can make AudioSink Send.
    cancel: CancellationToken,
    audio_thread: Option<std::thread::JoinHandle<Result<()>>>,
}

impl AudioSink {
    /// Create new audio sink.
    ///
    /// Input is a single channel (mono), with values between -1.0 and +1.0.
    pub fn new(src: ReadStream<Float>, sample_rate: u64) -> Result<Self> {
        Self::new_opts(src, sample_rate, None)
    }
    /// Create a builder.
    #[must_use]
    pub fn builder() -> AudioSinkBuilder {
        AudioSinkBuilder::default()
    }
    fn new_opts(src: ReadStream<Float>, sample_rate: u64, dev: Option<&str>) -> Result<Self> {
        let output = CpalOutput::new(sample_rate as u32, dev)?;
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();

        let audio_thread = std::thread::Builder::new()
            .name("audio_sink_stream".into())
            .spawn(move || {
                let _stream = match output.start() {
                    Err(e) => {
                        tx.send(Err(e)).expect("sending error");
                        return Ok(());
                    }
                    Ok((sender, stream)) => {
                        tx.send(Ok(sender)).expect("sending sender");
                        stream
                    }
                };
                while !c2.is_canceled() {
                    std::thread::park();
                }
                Ok(())
            })?;
        // Try to receive sender.
        let sender = {
            let s = match rx.recv() {
                Ok(s) => s,
                Err(e) => return Err(e.into()),
            };
            // Ensure stream started ok.
            match s {
                Ok(s) => Some(s),
                Err(e) => return Err(e),
            }
        };
        Ok(Self {
            src,
            sender,
            cancel,
            audio_thread: Some(audio_thread),
        })
    }
}

impl Drop for AudioSink {
    fn drop(&mut self) {
        self.cancel.cancel(); // Allows the thread to end.
        self.sender.take(); // Ends the stream to cpal.
        if let Some(handle) = self.audio_thread.take() {
            handle.thread().unpark();
            if let Err(e) = handle.join().expect("audio stream thread failed") {
                error!("Audio stream thread failed: {e}");
            }
        }
    }
}

impl Block for AudioSink {
    fn work(&mut self) -> Result<BlockRet> {
        let (i, _tags) = self.src.read_buf()?;
        let n = i.len();
        for (pos, x) in i.iter().enumerate() {
            if let Err(e) = self.sender.as_ref().unwrap().send(*x) {
                i.consume(pos);
                return Err(Error::msg(format!("audio error: {e}")));
            }
        }
        i.consume(n);

        Ok(BlockRet::WaitForStream(&self.src, 1))
    }
}
