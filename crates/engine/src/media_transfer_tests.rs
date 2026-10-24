use super::media_transfer::{TransferStage, transfer_file};
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, decode, encode};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sculpt-media-transfer-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn file_transfer_matches_legacy_src_for_88200_and_compressed_media() {
    let fixture = Fixture::new();
    let mut cases = Vec::new();
    for channels in [1, 2, 7, 64] {
        let source = AudioBuffer {
            sample_rate: 88_200,
            channels: (0..channels).map(|channel| (0..9003).map(|frame| (frame as f32 * 0.017 + channel as f32).sin() * 1.5).collect()).collect(),
        };
        cases.push((
            format!("{channels}ch.wav"),
            encode(&source, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap(),
        ));
    }
    let source = AudioBuffer { sample_rate: 96_000, channels: vec![vec![0.3; 9003]; 2] };
    for (format, extension) in [(FileFormat::Flac, "flac"), (FileFormat::Aiff, "aiff")] {
        cases.push((
            format!("input.{extension}"),
            encode(&source, &EncodeOptions { format, bit_depth: BitDepth::Int24, ..EncodeOptions::default() }).unwrap(),
        ));
    }
    cases.push(("gapless.mp3".into(), include_bytes!("../../audio-io/tests/fixtures/synthetic-stereo.mp3").to_vec()));
    cases.push(("no-xing.mp3".into(), include_bytes!("../../audio-io/tests/fixtures/synthetic-mono-no-xing.mp3").to_vec()));
    for (index, (name, bytes)) in cases.into_iter().enumerate() {
        let input = fixture.0.join(&name);
        let output = fixture.0.join(format!("converted-{index}.wav"));
        std::fs::write(&input, &bytes).unwrap();
        let mut progress = Vec::new();
        let receipt = transfer_file(&input, &output, 48_000, |p| {
            progress.push(p);
            true
        })
        .unwrap();
        let (_, original) = decode(&bytes, input.extension().and_then(|s| s.to_str())).unwrap();
        let expected = soundcraft_dsp::offline::resample(&original.channels, original.sample_rate, 48_000);
        let converted_bytes = std::fs::read(&output).unwrap();
        let (info, actual) = decode(&converted_bytes, Some("wav")).unwrap();
        assert_eq!(actual.channels, expected, "{name}");
        assert_eq!(actual.sample_rate, 48_000);
        assert_eq!(receipt.output_frames, actual.frames() as u64);
        assert_eq!(receipt.source.sample_rate, original.sample_rate);
        assert_eq!(receipt.source.frames, original.frames() as u64);
        assert_eq!(receipt.output_bytes, converted_bytes.len() as u64);
        assert_eq!(info.frames, receipt.output_frames);
        assert_eq!(progress.last().unwrap().completed, receipt.output_frames);
        if original.sample_rate != 48_000 {
            assert!(progress.iter().any(|p| p.stage == TransferStage::Resample));
        }
        assert_eq!(std::fs::read(&input).unwrap(), bytes);
        assert!(transfer_file(&input, &output, 48_000, |_| true).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), converted_bytes);
        assert!(transfer_file(&input, &input, 48_000, |_| true).is_err());
        assert_eq!(std::fs::read(&input).unwrap(), bytes);
    }
    assert!(std::fs::read_dir(&fixture.0).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().ends_with(".partial")));
}

#[test]
fn cancellation_in_each_stage_removes_only_generated_files() {
    let fixture = Fixture::new();
    let input = fixture.0.join("original.wav");
    let bytes = encode(&AudioBuffer::new(88_200, 2, 30003), &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
    std::fs::write(&input, &bytes).unwrap();
    for mode in 0..3 {
        let output = fixture.0.join(format!("cancelled-{mode}.wav"));
        assert!(
            transfer_file(&input, &output, 48_000, |p| match mode {
                0 => p.stage != TransferStage::Decode || p.completed == 0,
                1 => p.stage != TransferStage::Resample,
                _ => p.stage != TransferStage::Resample || p.completed == 0,
            })
            .is_err()
        );
        assert!(!output.exists());
        assert_eq!(std::fs::read(&input).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }
}

#[test]
fn corrupt_media_and_invalid_destination_do_not_leave_outputs() {
    let fixture = Fixture::new();
    let input = fixture.0.join("input.wav");
    let bytes = encode(&AudioBuffer::new(88_200, 2, 75003), &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
    std::fs::write(&input, &bytes).unwrap();
    let output = fixture.0.join("failed.wav");
    let mut changed = false;
    assert!(
        transfer_file(&input, &output, 48_000, |p| {
            if p.stage == TransferStage::Decode && p.completed > 0 && !changed {
                std::fs::OpenOptions::new().write(true).open(&input).unwrap().set_len(100).unwrap();
                changed = true;
            }
            true
        })
        .is_err()
    );
    assert!(changed);
    assert!(!output.exists());
    assert!(input.exists());
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    std::fs::write(&input, b"invalid audio").unwrap();
    assert!(transfer_file(&input, &output, 48_000, |_| true).is_err());
    assert!(!output.exists());
    assert!(transfer_file(&input, &output, 0, |_| true).is_err());
    assert!(!output.exists());
}
