#![cfg(not(target_arch = "wasm32"))]

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, encode, stream_write::FloatWavWriter};

#[test]
fn partial_write_and_flush_errors_are_not_successful_transfers() {
    use std::io::{self, Write};
    struct Sink {
        remaining: usize,
        fail_flush: bool,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("injected disk failure"));
            }
            let n = self.remaining.min(bytes.len());
            self.remaining -= n;
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush { Err(io::Error::other("injected flush failure")) } else { Ok(()) }
        }
    }
    let header = soundcraft_audio_io::wav_float_header(1, 48_000, 2).unwrap().len();
    assert!(FloatWavWriter::new(Sink { remaining: 2, fail_flush: false }, 1, 48_000, 2).is_err());
    let mut writer = FloatWavWriter::new(Sink { remaining: header + 3, fail_flush: false }, 1, 48_000, 2).unwrap();
    let block = AudioBuffer::new(48_000, 1, 2);
    assert!(writer.write_frames(&block).is_err());
    assert_eq!(writer.frames_written(), 0);
    assert!(writer.write_frames(&block).is_err());
    assert!(writer.finish().is_err());
    let mut writer = FloatWavWriter::new(Sink { remaining: usize::MAX, fail_flush: true }, 1, 48_000, 2).unwrap();
    writer.write_frames(&block).unwrap();
    assert!(writer.finish().is_err());
}

#[test]
fn bounded_blocks_match_float_encoder_exactly() {
    for channels in [1, 2, 7, 64] {
        let source = AudioBuffer {
            sample_rate: 96_000,
            channels: (0..channels).map(|c| (0..4123).map(|f| (f as f32 - 2000.0) / (c + 1) as f32).collect()).collect(),
        };
        let expected = encode(&source, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
        let mut writer = FloatWavWriter::new(Vec::new(), channels, 96_000, 4123).unwrap();
        for start in (0..4123).step_by(333) {
            let end = (start + 333).min(4123);
            writer
                .write_frames(&AudioBuffer { sample_rate: 96_000, channels: source.channels.iter().map(|c| c[start..end].to_vec()).collect() })
                .unwrap();
        }
        assert_eq!(writer.frames_written(), 4123);
        assert_eq!(writer.finish().unwrap(), expected);
    }
}

#[test]
fn malformed_or_excess_blocks_do_not_advance_and_short_finish_fails() {
    let mut writer = FloatWavWriter::new(Vec::new(), 2, 48_000, 2).unwrap();
    for block in [
        AudioBuffer::new(44_100, 2, 2),
        AudioBuffer::new(48_000, 1, 2),
        AudioBuffer::new(48_000, 2, 3),
        AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 2], vec![0.0]] },
    ] {
        assert!(writer.write_frames(&block).is_err());
        assert_eq!(writer.frames_written(), 0);
    }
    writer.write_frames(&AudioBuffer::new(48_000, 2, 1)).unwrap();
    assert!(writer.finish().is_err());
    assert!(FloatWavWriter::new(Vec::new(), 65, 48_000, 0).is_err());
    assert!(FloatWavWriter::new(Vec::new(), 0, 48_000, 0).is_err());
    assert!(FloatWavWriter::new(Vec::new(), 2, 0, 0).is_err());
    assert!(FloatWavWriter::new(Vec::new(), 64, 48_000, u64::MAX).is_err());
}

#[test]
fn rf64_header_is_bounded_and_nonfinite_policy_matches_encoder() {
    let mut sink = Vec::new();
    let writer = FloatWavWriter::new(&mut sink, 7, 192_000, 1_000_000_000).unwrap();
    assert!(writer.finish().is_err());
    assert_eq!(&sink[..4], b"RF64");
    assert!(sink.len() < 1024);
    let source = AudioBuffer { sample_rate: 48_000, channels: vec![vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.0, 2.0]] };
    let mut writer = FloatWavWriter::new(Vec::new(), 1, 48_000, 5).unwrap();
    writer.write_frames(&source).unwrap();
    assert_eq!(writer.finish().unwrap(), encode(&source, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap());
}
