#![cfg(not(target_arch = "wasm32"))]
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, decode, encode, stream_write::GrowingFloatWavWriter};
use std::io::Cursor;

#[test]
fn growing_wave_matches_known_length_encoder_across_block_boundaries() {
    for channels in [1, 2, 7, 64] {
        let source = AudioBuffer {
            sample_rate: 96_000,
            channels: (0..channels).map(|ch| (0..4137).map(|f| (f as f32 - 2000.0) / (ch + 1) as f32).collect()).collect(),
        };
        let mut writer = GrowingFloatWavWriter::new(Cursor::new(Vec::new()), channels, 96_000).unwrap();
        for start in (0..4137).step_by(317) {
            let end = (start + 317).min(4137);
            writer
                .write_frames(&AudioBuffer { sample_rate: 96_000, channels: source.channels.iter().map(|ch| ch[start..end].to_vec()).collect() })
                .unwrap();
        }
        assert_eq!(writer.frames_written(), 4137);
        let output = writer.finish().unwrap();
        assert_eq!(output.position(), output.get_ref().len() as u64);
        assert_eq!(output.into_inner(), encode(&source, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap());
    }
    let empty = GrowingFloatWavWriter::new(Cursor::new(Vec::new()), 2, 48_000).unwrap().finish().unwrap().into_inner();
    assert_eq!(decode(&empty, None).unwrap().1.frames(), 0);
}

#[test]
fn growing_wave_rejects_existing_output_and_latches_invalid_blocks() {
    let mut existing = Cursor::new(vec![1, 2, 3]);
    assert!(GrowingFloatWavWriter::new(&mut existing, 1, 48_000).is_err());
    assert_eq!(existing.into_inner(), vec![1, 2, 3]);
    let mut writer = GrowingFloatWavWriter::new(Cursor::new(Vec::new()), 1, 48_000).unwrap();
    assert!(writer.write_frames(&AudioBuffer::new(44_100, 1, 10)).is_err());
    assert!(writer.write_frames(&AudioBuffer::new(48_000, 1, 10)).is_err());
    assert!(writer.finish().is_err());
}

#[test]
fn growing_wave_propagates_write_seek_and_flush_failures() {
    use std::{
        cell::Cell,
        io::{self, Seek, SeekFrom, Write},
        rc::Rc,
    };
    struct Faults {
        write: Cell<bool>,
        seek: Cell<bool>,
        flush: Cell<bool>,
    }
    struct Destination {
        cursor: Cursor<Vec<u8>>,
        faults: Rc<Faults>,
    }
    impl Write for Destination {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            if self.faults.write.get() {
                return Err(io::Error::other("injected write error"));
            }
            self.cursor.write(data)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.faults.flush.get() { Err(io::Error::other("injected flush error")) } else { Ok(()) }
        }
    }
    impl Seek for Destination {
        fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
            if self.faults.seek.get() {
                return Err(io::Error::other("injected seek error"));
            }
            self.cursor.seek(from)
        }
    }
    for fault in 0..4 {
        let faults = Rc::new(Faults { write: Cell::new(false), seek: Cell::new(false), flush: Cell::new(false) });
        let mut writer = GrowingFloatWavWriter::new(Destination { cursor: Cursor::new(Vec::new()), faults: Rc::clone(&faults) }, 1, 48_000).unwrap();
        if fault == 0 {
            faults.write.set(true);
            assert!(writer.write_frames(&AudioBuffer::new(48_000, 1, 10)).is_err());
            faults.write.set(false);
        } else {
            writer.write_frames(&AudioBuffer::new(48_000, 1, 10)).unwrap();
            match fault {
                1 => faults.write.set(true),
                2 => faults.seek.set(true),
                _ => faults.flush.set(true),
            }
        }
        assert!(writer.finish().is_err());
    }
}
