#![cfg(not(target_arch = "wasm32"))]
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, decode, decode_stream::FileDecoder, encode};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(bytes: &[u8], extension: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "sculpt-decode-stream-{}-{}.{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed),
            extension
        ));
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn read_all(path: &std::path::Path) -> AudioBuffer {
    let mut decoder = FileDecoder::open(path).unwrap();
    let mut result = AudioBuffer { sample_rate: decoder.info().sample_rate, channels: Vec::new() };
    let mut blocks = 0;
    while let Some(block) = decoder.next_block().unwrap() {
        if result.channels.is_empty() {
            result.channels = (0..block.num_channels()).map(|_| Vec::new()).collect();
        }
        assert!(block.frames() * block.num_channels() <= 1_048_576);
        for (dst, src) in result.channels.iter_mut().zip(block.channels) {
            dst.extend(src);
        }
        blocks += 1;
    }
    assert!(blocks > 1);
    assert_eq!(decoder.frames_decoded(), result.frames() as u64);
    assert_eq!(decoder.info().frames, result.frames() as u64);
    assert!(decoder.next_block().unwrap().is_none());
    result
}

#[test]
fn wav_aiff_flac_blocks_match_existing_decoders_without_whole_file_staging() {
    for channels in [1, 2, 7] {
        let source = AudioBuffer {
            sample_rate: 88_200,
            channels: (0..channels).map(|ch| (0..75003).map(|f| (f as f32 * 0.017 + ch as f32).sin() * 1.5).collect()).collect(),
        };
        for (format, extension, depth) in
            [(FileFormat::Wav, "wav", BitDepth::Float32), (FileFormat::Aiff, "aiff", BitDepth::Float32), (FileFormat::Flac, "flac", BitDepth::Int24)]
        {
            let bytes = encode(&source, &EncodeOptions { format, bit_depth: depth, dither: false, ..EncodeOptions::default() }).unwrap();
            let fixture = Fixture::new(&bytes, extension);
            assert_eq!(read_all(&fixture.0), decode(&bytes, Some(extension)).unwrap().1, "{format:?} {channels}ch");
        }
    }
}

#[test]
fn rf64_and_bw64_keep_the_native_reader_and_exact_samples() {
    let source = AudioBuffer { sample_rate: 96_000, channels: vec![vec![1.5; 75003]; 7] };
    let mut bytes =
        soundcraft_audio_io::encode_wav_rf64(&source, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
    for signature in [b"RF64", b"BW64"] {
        bytes[..4].copy_from_slice(signature);
        let fixture = Fixture::new(&bytes, "wav");
        assert_eq!(read_all(&fixture.0), source);
    }
}

#[test]
fn synthetic_gapless_mp3_preserves_length_stereo_and_existing_decode_samples() {
    let bytes = include_bytes!("fixtures/synthetic-stereo.mp3");
    let fixture = Fixture::new(bytes, "mp3");
    let actual = read_all(&fixture.0);
    let expected = decode(bytes, Some("mp3")).unwrap().1;
    assert_eq!(actual, expected);
    assert_eq!(actual.frames(), 5292);
    assert_eq!(actual.sample_rate, 44_100);
    assert_eq!(actual.num_channels(), 2);
    assert_ne!(actual.channels[0], actual.channels[1]);
    assert!(actual.peak() > 0.3);
}

#[test]
fn mp3_without_xing_counts_until_eof_and_preserves_existing_mono_decode() {
    let bytes = include_bytes!("fixtures/synthetic-mono-no-xing.mp3");
    let fixture = Fixture::new(bytes, "mp3");
    let actual = read_all(&fixture.0);
    assert_eq!(actual, decode(bytes, Some("mp3")).unwrap().1);
    assert_eq!(actual.sample_rate, 48_000);
    assert_eq!(actual.num_channels(), 1);
    assert!(actual.frames() >= 5760);
    assert!(actual.peak() > 0.1);
}

#[test]
fn decoded_mp3_transfers_to_float_wave_and_cancellation_leaves_unfinalized_output() {
    use soundcraft_audio_io::decode_stream::decode_file_to_wave;
    use std::io::Cursor;
    for bytes in [include_bytes!("fixtures/synthetic-stereo.mp3").as_slice(), include_bytes!("fixtures/synthetic-mono-no-xing.mp3").as_slice()] {
        let fixture = Fixture::new(bytes, "mp3");
        let mut output = Cursor::new(Vec::new());
        let mut progress = Vec::new();
        let (_, info) = decode_file_to_wave(&fixture.0, &mut output, |done, total| {
            progress.push((done, total));
            true
        })
        .unwrap();
        let expected = decode(bytes, Some("mp3")).unwrap().1;
        assert_eq!(decode(output.get_ref(), Some("wav")).unwrap().1, expected);
        assert_eq!(info.frames, expected.frames() as u64);
        assert_eq!(info.sample_rate, expected.sample_rate);
        assert_eq!(progress.first().unwrap().0, 0);
        assert_eq!(progress.last().unwrap().0, info.frames);
        let mut early = Cursor::new(Vec::new());
        assert!(decode_file_to_wave(&fixture.0, &mut early, |_, _| false).is_err());
        assert!(early.into_inner().is_empty());
        let mut late = Cursor::new(Vec::new());
        assert!(decode_file_to_wave(&fixture.0, &mut late, |done, _| done == 0).is_err());
        let reader = soundcraft_audio_io::stream::WavReader::from_reader(Cursor::new(late.into_inner())).unwrap();
        assert_eq!(reader.info().frames, 0, "cancelled output must not claim a completed duration");
    }
}

#[test]
fn decoder_failure_does_not_finalize_a_partial_wave_transfer() {
    use soundcraft_audio_io::decode_stream::decode_file_to_wave;
    use std::io::Cursor;
    let bytes = encode(&AudioBuffer::new(48_000, 2, 75003), &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
    let fixture = Fixture::new(&bytes, "wav");
    let mut output = Cursor::new(Vec::new());
    let mut changed = false;
    assert!(
        decode_file_to_wave(&fixture.0, &mut output, |done, _| {
            if done > 0 && !changed {
                std::fs::OpenOptions::new().write(true).open(&fixture.0).unwrap().set_len(100).unwrap();
                changed = true;
            }
            true
        })
        .is_err()
    );
    assert!(changed);
    assert_eq!(soundcraft_audio_io::stream::WavReader::from_reader(Cursor::new(output.into_inner())).unwrap().info().frames, 0);
}

#[test]
fn changed_truncated_and_invalid_files_fail_without_clean_eof() {
    let source = AudioBuffer::new(48_000, 2, 75003);
    let bytes = encode(&source, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
    let fixture = Fixture::new(&bytes, "wav");
    let mut decoder = FileDecoder::open(&fixture.0).unwrap();
    std::fs::OpenOptions::new().write(true).open(&fixture.0).unwrap().set_len(100).unwrap();
    let mut failed = false;
    for _ in 0..100 {
        match decoder.next_block() {
            Ok(Some(_)) => {}
            Err(_) => {
                failed = true;
                break;
            }
            Ok(None) => panic!("changed media returned clean EOF"),
        }
    }
    assert!(failed);
    assert!(decoder.next_block().is_err());
    let truncated = Fixture::new(&bytes[..bytes.len() / 2], "wav");
    if let Ok(mut decoder) = FileDecoder::open(&truncated.0) {
        loop {
            match decoder.next_block() {
                Ok(Some(_)) => {}
                Err(_) => break,
                Ok(None) => panic!("truncated media returned clean EOF"),
            }
        }
    }
    let junk = Fixture::new(b"not an audio file", "mp3");
    assert!(FileDecoder::open(&junk.0).is_err());
}

#[test]
fn mutated_and_short_mp3_inputs_finish_or_fail_without_panics() {
    let original = include_bytes!("fixtures/synthetic-stereo.mp3");
    for variant in 0..48usize {
        let mut bytes = original.to_vec();
        if variant % 2 == 0 {
            bytes.truncate(variant * 59);
        } else {
            for offset in (variant..bytes.len()).step_by(127) {
                bytes[offset] ^= 0xff;
            }
        }
        let fixture = Fixture::new(&bytes, "mp3");
        if let Ok(mut decoder) = FileDecoder::open(&fixture.0) {
            let mut terminal = false;
            for _ in 0..100 {
                match decoder.next_block() {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => {
                        terminal = true;
                        break;
                    }
                }
            }
            assert!(terminal);
        }
    }
}
