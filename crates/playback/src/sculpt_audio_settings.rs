//! SCULPT_CORE22_SHARED_AUDIO: a simple atomic shared audio profile for Show and DAW.
//! Both apps read ~/Library/Application Support/Sculpt/AudioSettings.conf.
//! Hardware UID is authoritative for Show; DAW matches the physical output's
//! unique display name through CPAL 0.16 (which does not expose a UID API).
//! If names are duplicated, DAW refuses the switch instead of guessing.
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioSettings {
    pub output_uid: String,
    pub output_name: String,
    pub input_name: String,
    pub sample_rate: u32,
    pub buffer_frames: u32,
}
impl AudioSettings {
    pub fn file() -> Option<PathBuf> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::env::var_os("HOME").map(PathBuf::from).map(|h| h.join("Library/Application Support/Sculpt/AudioSettings.conf"))
        }
    }
    pub fn read() -> Self {
        let Some(file) = Self::file() else { return Self::default() };
        let Ok(s) = std::fs::read_to_string(file) else { return Self::default() };
        let mut data = Self::default();
        if s.lines().next() != Some("SCULPT_AUDIO_V1") {
            return data;
        }
        for line in s.lines().skip(1) {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key {
                "uid" => data.output_uid = value.to_string(),
                "name" => data.output_name = value.to_string(),
                "input" => data.input_name = value.to_string(),
                "rate" => data.sample_rate = value.parse().unwrap_or(0),
                "buffer" => data.buffer_frames = value.parse().unwrap_or(0),
                _ => (),
            }
        }
        data
    }
    pub fn save(&self) -> Result<(), String> {
        let path = Self::file().ok_or("HOME not set")?;
        let dir = path.parent().ok_or("missing config parent")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let clean = |s: &str| s.replace(['\r', '\n'], " ");
        let body = format!(
            "SCULPT_AUDIO_V1\nuid={}\nname={}\ninput={}\nrate={}\nbuffer={}\n",
            clean(&self.output_uid),
            clean(&self.output_name),
            clean(&self.input_name),
            self.sample_rate,
            self.buffer_frames
        );
        let tmp = path.with_extension("conf.tmp");
        std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

/// Capabilities stay grouped: a rate and buffer must be supported by the same configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioConfigRange {
    pub channels: u16,
    pub min_rate: u32,
    pub max_rate: u32,
    pub buffer_range: Option<(u32, u32)>,
}
impl AudioConfigRange {
    pub fn supports(&self, rate: u32, buffer: u32) -> bool {
        self.channels > 0
            && self.channels <= 64
            && (self.min_rate..=self.max_rate).contains(&rate)
            && (buffer == 0 || ((16..=8192).contains(&buffer) && self.buffer_range.is_none_or(|(min, max)| (min..=max).contains(&buffer))))
    }
}

#[derive(Clone, Debug)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub channels: u16,
    pub is_default: bool,
    pub configs: Vec<AudioConfigRange>,
}
impl AudioDeviceInfo {
    pub fn sample_rates(&self) -> Vec<u32> {
        [44100, 48000, 88200, 96000, 176400, 192000].into_iter().filter(|rate| self.configs.iter().any(|c| c.supports(*rate, 0))).collect()
    }
    pub fn buffer_sizes(&self, rate: u32) -> Vec<u32> {
        [16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192]
            .into_iter()
            .filter(|buffer| self.configs.iter().any(|c| c.supports(rate, *buffer)))
            .collect()
    }
}

/// Probe only when opening the settings window or explicitly refreshing it.
pub fn available_devices(output: bool) -> Vec<AudioDeviceInfo> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default = if output { host.default_output_device() } else { host.default_input_device() };
    #[allow(deprecated)]
    let default_name = default.and_then(|d| d.name().ok());
    let devices = if output { host.output_devices() } else { host.input_devices() };
    let Ok(devices) = devices else { return Vec::new() };
    devices
        .filter_map(|d| {
            #[allow(deprecated)]
            let name = d.name().ok()?;
            let configs: Vec<_> = if output { d.supported_output_configs().ok()?.collect() } else { d.supported_input_configs().ok()?.collect() };
            let configs: Vec<_> = configs
                .into_iter()
                .filter(|c| c.sample_format() == cpal::SampleFormat::F32 && (1..=64).contains(&c.channels()))
                .map(|c| AudioConfigRange {
                    channels: c.channels(),
                    min_rate: c.min_sample_rate().0,
                    max_rate: c.max_sample_rate().0,
                    buffer_range: match c.buffer_size() {
                        cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
                        _ => None,
                    },
                })
                .collect();
            let channels = configs.iter().map(|c| c.channels).max().unwrap_or(0);
            (channels > 0).then(|| AudioDeviceInfo { is_default: default_name.as_deref() == Some(name.as_str()), name, channels, configs })
        })
        .collect()
}

pub fn available_output_devices() -> Vec<AudioDeviceInfo> {
    available_devices(true)
}

/// Resolve explicit requests strictly. Never silently change a chosen rate or buffer.
pub(crate) fn stream_config(device: &cpal::Device, output: bool, rate: u32, buffer: u32) -> Result<cpal::StreamConfig, String> {
    use cpal::traits::DeviceTrait;
    let ranges: Vec<_> = if output {
        device.supported_output_configs().map_err(|e| e.to_string())?.collect()
    } else {
        device.supported_input_configs().map_err(|e| e.to_string())?.collect()
    };
    let chosen = ranges
        .into_iter()
        .filter(|c| {
            let range = AudioConfigRange {
                channels: c.channels(),
                min_rate: c.min_sample_rate().0,
                max_rate: c.max_sample_rate().0,
                buffer_range: match c.buffer_size() {
                    cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
                    _ => None,
                },
            };
            c.sample_format() == cpal::SampleFormat::F32 && range.supports(rate, buffer)
        })
        .max_by_key(cpal::SupportedStreamConfigRange::channels)
        .ok_or_else(|| {
            format!(
                "No supported {} configuration at {rate} Hz with buffer {}",
                if output { "output" } else { "input" },
                if buffer == 0 { "default".into() } else { buffer.to_string() }
            )
        })?;
    let mut config = chosen.with_sample_rate(cpal::SampleRate(rate)).config();
    config.buffer_size = if buffer == 0 { cpal::BufferSize::Default } else { cpal::BufferSize::Fixed(buffer) };
    Ok(config)
}

#[cfg(test)]
mod sculpt_core22_audio_tests {
    use super::*;
    #[test]
    fn defaults_do_not_force_a_device() {
        let p = AudioSettings::default();
        assert!(p.output_uid.is_empty());
        assert_eq!(p.sample_rate, 0);
    }
    #[test]
    fn capabilities_keep_rate_buffer_and_channels_together() {
        let low = AudioConfigRange { channels: 8, min_rate: 44100, max_rate: 48000, buffer_range: Some((128, 1024)) };
        let high = AudioConfigRange { channels: 2, min_rate: 96000, max_rate: 96000, buffer_range: Some((512, 2048)) };
        let device = AudioDeviceInfo { name: "test".into(), channels: 8, is_default: false, configs: vec![low, high] };
        assert_eq!(device.sample_rates(), [44100, 48000, 96000]);
        assert_eq!(device.buffer_sizes(96000), [512, 1024, 2048]);
        assert!(!device.configs.iter().any(|c| c.supports(96000, 128)));
        assert!(device.configs.iter().any(|c| c.supports(48000, 0)));
        assert!(device.buffer_sizes(192000).is_empty());
    }
    #[test]
    fn unknown_buffer_range_is_bounded_and_invalid_channel_counts_are_rejected() {
        let range = AudioConfigRange { channels: 64, min_rate: 48000, max_rate: 48000, buffer_range: None };
        assert!(range.supports(48000, 32));
        assert!(!range.supports(48000, 8193));
        assert!(!AudioConfigRange { channels: 65, ..range.clone() }.supports(48000, 0));
        assert!(!AudioConfigRange { channels: 0, ..range }.supports(48000, 0));
    }
}
