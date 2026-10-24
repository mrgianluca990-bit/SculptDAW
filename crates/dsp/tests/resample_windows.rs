use soundcraft_dsp::offline::{RESAMPLE_WINDOW_SAMPLES, WindowedResampler, resample};

fn blocked(input: &[Vec<f32>], from: u32, to: u32, block: usize) -> Vec<Vec<f32>> {
    let converter = WindowedResampler::new(from, to, input[0].len() as u64, input.len()).unwrap();
    let mut output = vec![Vec::new(); input.len()];
    let mut position = 0u64;
    while position < converter.output_frames() {
        let count = block.min(converter.recommended_block_frames()).min((converter.output_frames() - position) as usize);
        let range = converter.input_window(position, count).unwrap();
        let window: Vec<_> = input.iter().map(|channel| channel[range.start as usize..range.end as usize].to_vec()).collect();
        let converted = converter.process_window(&window, range.start, position, count).unwrap();
        for (out, channel) in output.iter_mut().zip(converted) {
            out.extend(channel);
        }
        position += count as u64;
    }
    output
}

#[test]
fn blocked_src_matches_existing_sinc_bit_for_bit_across_channels_and_rates() {
    for channels in [1, 2, 7, 64] {
        let length = if channels == 64 { 1003 } else { 9003 };
        let mut input: Vec<Vec<f32>> =
            (0..channels).map(|ch| (0..length).map(|frame| (frame as f32 * 0.017 + ch as f32).sin() * 1.5).collect()).collect();
        input[0][17] = -0.0;
        for (from, to) in [(44_100, 48_000), (88_200, 48_000), (192_000, 44_100), (48_000, 96_000), (48_000, 48_000)] {
            let actual = blocked(&input, from, to, 317);
            let expected = resample(&input, from, to);
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.iter().zip(expected) {
                assert_eq!(
                    actual.iter().map(|n| n.to_bits()).collect::<Vec<_>>(),
                    expected.iter().map(|n| n.to_bits()).collect::<Vec<_>>(),
                    "{channels}ch {from}→{to}"
                );
            }
        }
    }
}

#[test]
fn long_global_positions_keep_phase_independent_of_output_partition() {
    let converter = WindowedResampler::new(192_000, 48_000, 1u64 << 35, 2).unwrap();
    assert_eq!(converter.output_frames(), 1u64 << 33);
    let start = (1u64 << 32) + 137;
    let range = converter.input_window(start, 64).unwrap();
    assert!(range.start > u64::from(u32::MAX));
    let input: Vec<_> = (0..2).map(|ch| range.clone().map(|index| ((index % 997) as f32 * 0.01 + ch as f32).sin()).collect()).collect();
    let all = converter.process_window(&input, range.start, start, 64).unwrap();
    let first = converter.process_window(&input, range.start, start, 17).unwrap();
    let second = converter.process_window(&input, range.start, start + 17, 47).unwrap();
    for channel in 0..2 {
        assert_eq!(all[channel], first[channel].iter().chain(&second[channel]).copied().collect::<Vec<_>>());
    }
}

#[test]
fn impulse_stays_on_its_channel_and_input_errors_never_pad_missing_media() {
    let mut input = vec![vec![0.0; 9003]; 2];
    input[0][4000] = 0.75;
    let converted = blocked(&input, 88_200, 48_000, 257);
    assert!(converted[1].iter().all(|v| *v == 0.0));
    let peak = converted[0].iter().enumerate().max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs())).unwrap().0;
    assert_eq!(peak, 2177);
    let converter = WindowedResampler::new(88_200, 48_000, 9003, 2).unwrap();
    let range = converter.input_window(100, 64).unwrap();
    let complete: Vec<_> = input.iter().map(|c| c[range.start as usize..range.end as usize].to_vec()).collect();
    let mut short = complete.clone();
    for channel in &mut short {
        channel.pop();
    }
    assert!(converter.process_window(&short, range.start, 100, 64).is_err());
    assert!(converter.process_window(&complete, range.start + 1, 100, 64).is_err());
    let mut invalid = complete.clone();
    invalid[0][0] = f32::NAN;
    assert!(converter.process_window(&invalid, range.start, 100, 64).is_err());
    assert!(converter.input_window(converter.output_frames(), 1).is_err());
    assert!(converter.input_window(0, RESAMPLE_WINDOW_SAMPLES).is_err());
    assert!(WindowedResampler::new(0, 48_000, 100, 2).is_err());
    assert!(WindowedResampler::new(48_000, 0, 100, 2).is_err());
    assert!(WindowedResampler::new(48_000, 48_000, u64::MAX, 2).is_err());
    assert!(WindowedResampler::new(48_000, 48_000, 100, 65).is_err());
}

#[test]
fn recommended_windows_fit_budgets_at_rate_extremes_and_empty_input_is_explicit() {
    for channels in [1, 7, 64] {
        for (from, to) in [(192_000, 8_000), (8_000, 192_000), (384_000, 44_100), (44_100, 384_000), (768_000, 8_000)] {
            let converter = WindowedResampler::new(from, to, 1_000_003, channels).unwrap();
            let count = converter.recommended_block_frames().min(converter.output_frames() as usize);
            for position in [0, converter.output_frames() - count as u64] {
                let range = converter.input_window(position, count).unwrap();
                assert!(range.end <= 1_000_003);
                assert!(range.end - range.start <= 65_536);
                assert!((range.end - range.start) * channels as u64 <= RESAMPLE_WINDOW_SAMPLES as u64);
            }
        }
    }
    let empty = WindowedResampler::new(88_200, 48_000, 0, 2).unwrap();
    assert_eq!(empty.output_frames(), 0);
    assert_eq!(empty.input_window(0, 0).unwrap(), 0..0);
    assert_eq!(empty.process_window(&[Vec::new(), Vec::new()], 0, 0, 0).unwrap(), vec![Vec::<f32>::new(), Vec::new()]);
    assert!(empty.input_window(0, 1).is_err());
}
