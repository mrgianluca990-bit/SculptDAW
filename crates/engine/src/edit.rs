//! Editing algorithms on playlists, independent of the command layer.
//!
//! Model rules: clips on a playlist are sorted and do not overlap, except where two clips
//! overlap exactly over a crossfade (the earlier clip fades out while the later fades in).
//! Placing a clip overwrites whatever is under it (like the incumbent's Slip mode); Shuffle
//! mode also ripples later clips.

use soundcraft_model::{Clip, ClipContent, ClipId, Fade, FadeShape, Session, TrackId};
use soundcraft_time::{Range, SampleRate, Samples, TempoMap};

/// Context needed to split MIDI clips in musical time.
#[derive(Clone, Copy)]
pub struct TimeCtx<'a> {
    pub tempo: &'a TempoMap,
    pub sr: SampleRate,
}

impl<'a> TimeCtx<'a> {
    pub fn of(s: &'a Session) -> Self {
        TimeCtx { tempo: &s.tempo, sr: s.sample_rate }
    }
    fn ticks(&self, s: Samples) -> i64 {
        self.tempo.samples_to_ticks(s, self.sr)
    }
}

/// Split `clip` at absolute position `at` (strictly inside). Returns (left, right) where right gets `new_id`.
pub fn split_clip(clip: &Clip, at: Samples, new_id: ClipId, t: TimeCtx<'_>) -> Option<(Clip, Clip)> {
    if at <= clip.start || at >= clip.end() {
        return None;
    }
    let mut left = clip.clone();
    let mut right = clip.clone();
    right.id = new_id;
    left.trim_end_to(at);
    right.trim_start_to(at);
    left.fade_out = Fade::default();
    right.fade_in = Fade::default();
    if let (ClipContent::Midi { sequence: ls }, ClipContent::Midi { sequence: rs }) = (&mut left.content, &mut right.content) {
        let boundary = t.ticks(at) - t.ticks(clip.start);
        ls.notes.retain(|n| n.start < boundary);
        ls.ctrls.retain(|c| c.tick < boundary);
        rs.notes.retain(|n| n.start >= boundary);
        rs.ctrls.retain(|c| c.tick >= boundary);
        for n in &mut rs.notes {
            n.start -= boundary;
        }
        for c in &mut rs.ctrls {
            c.tick -= boundary;
        }
    }
    Some((left, right))
}

/// Trim a MIDI clip's start: drop notes before the new start and re-anchor the rest.
fn rebase_midi_start(clip: &mut Clip, old_start: Samples, t: TimeCtx<'_>) {
    let delta = t.ticks(clip.start) - t.ticks(old_start);
    if let ClipContent::Midi { sequence } = &mut clip.content {
        if delta == 0 {
            return;
        }
        sequence.notes.retain(|n| n.start >= delta);
        sequence.ctrls.retain(|c| c.tick >= delta);
        for n in &mut sequence.notes {
            n.start -= delta;
        }
        for c in &mut sequence.ctrls {
            c.tick -= delta;
        }
    }
}

/// Split every clip on a track's active playlist that spans `at`.
pub fn separate_at(s: &mut Session, track: TrackId, at: Samples) -> usize {
    let tc_tempo = s.tempo.clone();
    let sr = s.sample_rate;
    let t = TimeCtx { tempo: &tc_tempo, sr };
    let mut ids = Vec::new();
    let n_split = s.track(track).map_or(0, |tr| tr.clips().iter().filter(|c| at > c.start && at < c.end()).count());
    for _ in 0..n_split {
        ids.push(s.new_clip_id());
    }
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut out = Vec::with_capacity(pl.clips.len() + ids.len());
    let mut count = 0;
    for c in pl.clips.drain(..) {
        if at > c.start
            && at < c.end()
            && let Some(id) = ids.pop()
            && let Some((l, r)) = split_clip(&c, at, id, t)
        {
            out.push(l);
            out.push(r);
            count += 1;
            continue;
        }
        out.push(c);
    }
    pl.clips = out;
    pl.sort();
    count
}

/// Remove material inside `range` on a track. In shuffle mode later clips move left.
/// Returns the removed material as clips positioned relative to `range.start`.
pub fn clear_range(s: &mut Session, track: TrackId, range: Range, shuffle: bool) -> Vec<Clip> {
    if range.is_empty() {
        return Vec::new();
    }
    separate_at(s, track, range.start);
    separate_at(s, track, range.end);
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return Vec::new() };
    let mut removed = Vec::new();
    pl.clips.retain(|c| {
        if c.start >= range.start && c.end() <= range.end {
            let mut r = c.clone();
            r.start -= range.start;
            removed.push(r);
            false
        } else {
            true
        }
    });
    if shuffle {
        let len = range.len();
        for c in &mut pl.clips {
            if c.start >= range.end {
                c.start -= len;
            }
        }
    }
    pl.sort();
    removed
}

/// Copy the material inside `range` (clips trimmed to the range), relative to `range.start`.
pub fn copy_range(s: &Session, track: TrackId, range: Range) -> Vec<Clip> {
    let t = TimeCtx::of(s);
    let Some(tr) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    for c in tr.clips() {
        let Some(isect) = c.range().intersect(&range) else { continue };
        let mut part = c.clone();
        if isect.start > part.start {
            let old = part.start;
            part.trim_start_to(isect.start);
            rebase_midi_start(&mut part, old, t);
        }
        if isect.end < part.end() {
            part.trim_end_to(isect.end);
        }
        part.start -= range.start;
        out.push(part);
    }
    out
}

/// Insert `clips` (positions relative to 0) at `at`. Overwrites what's underneath unless
/// `shuffle`, which first pushes later material right by `length`.
pub fn paste(s: &mut Session, track: TrackId, at: Samples, clips: &[Clip], length: Samples, shuffle: bool) -> Vec<ClipId> {
    let length = length.max(clips.iter().map(Clip::end).max().unwrap_or(0));
    if shuffle {
        separate_at(s, track, at);
        if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
            for c in &mut pl.clips {
                if c.start >= at {
                    c.start += length;
                }
            }
        }
    }
    let mut ids = Vec::new();
    for c in clips {
        let mut nc = c.clone();
        nc.id = s.new_clip_id();
        nc.start += at;
        ids.push(nc.id);
        place_clip(s, track, nc);
    }
    ids
}

/// Place one clip, overwriting whatever it covers.
pub fn place_clip(s: &mut Session, track: TrackId, clip: Clip) {
    clear_range(s, track, clip.range(), false);
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        pl.clips.push(clip);
        pl.sort();
    }
}

/// Insert silence: shift material at/after `range.start` right by the range length.
pub fn insert_silence(s: &mut Session, track: TrackId, range: Range) {
    if range.is_empty() {
        return;
    }
    separate_at(s, track, range.start);
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        for c in &mut pl.clips {
            if c.start >= range.start {
                c.start += range.len();
            }
        }
    }
}

/// Trim clips to keep only material inside `range`.
pub fn trim_to(s: &mut Session, track: TrackId, range: Range) -> usize {
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        if let Some(i) = c.range().intersect(&range)
            && i != c.range()
        {
            let old = c.start;
            c.trim_start_to(i.start);
            rebase_midi_start(c, old, tc);
            c.trim_end_to(i.end);
            n += 1;
        }
    }
    n
}

/// Trim the start of clips under `at` to `at` (Trim Start to Insertion).
pub fn trim_start_to(s: &mut Session, track: TrackId, at: Samples) -> usize {
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        if c.range().contains(at) && at > c.start {
            let old = c.start;
            c.trim_start_to(at);
            rebase_midi_start(c, old, tc);
            n += 1;
        }
    }
    n
}

pub fn trim_end_to(s: &mut Session, track: TrackId, at: Samples) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        if at > c.start && at < c.end() {
            c.trim_end_to(at);
            n += 1;
        }
    }
    n
}

/// Heal separations inside `range`: merge neighbouring audio clips that continue the same file.
pub fn heal(s: &mut Session, track: TrackId, range: Range) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    pl.sort();
    let mut out: Vec<Clip> = Vec::with_capacity(pl.clips.len());
    let mut healed = 0;
    for c in pl.clips.drain(..) {
        if let Some(prev) = out.last_mut() {
            let contiguous = prev.end() == c.start && c.start >= range.start && c.start <= range.end;
            let same = match (&prev.content, &c.content) {
                (ClipContent::Audio { source: a, offset: oa }, ClipContent::Audio { source: b, offset: ob }) => a == b && oa + prev.length == *ob,
                _ => false,
            };
            if contiguous && same && (prev.gain_db - c.gain_db).abs() < 1e-6 {
                prev.length += c.length;
                prev.fade_out = c.fade_out;
                healed += 1;
                continue;
            }
        }
        out.push(c);
    }
    pl.clips = out;
    healed
}

/// Move clips by `delta` (and optionally to another track), overwriting what they land on.
pub fn move_clips(s: &mut Session, ids: &[ClipId], delta: Samples, to_track: Option<TrackId>) -> usize {
    let mut moving: Vec<(TrackId, Clip)> = Vec::new();
    for t in &mut s.tracks {
        let tid = t.id;
        if let Some(pl) = t.playlist_mut() {
            pl.clips.retain(|c| {
                if ids.contains(&c.id) && !c.time_locked && !c.edit_locked {
                    moving.push((tid, c.clone()));
                    false
                } else {
                    true
                }
            });
        }
    }
    let n = moving.len();
    for (tid, mut c) in moving {
        c.start = c.start.saturating_add(delta).max(0);
        place_clip(s, to_track.unwrap_or(tid), c);
    }
    n
}

/// Session-stable original cut location for a crossfade between these two clip IDs.
/// Stored in the undoable session edit state; no irreversible "split at midpoint" deletion.
fn sculpt_xfade_join_key(a: ClipId, b: ClipId) -> String {
    format!("sculpt.xfade.join.{}.{}", a.0, b.0)
}

/// Create fades from a selection (the incumbent's Edit › Fades › Create semantics):
/// a range across a clip boundary makes a crossfade; at a clip start a fade-in; at its end a fade-out.
pub fn create_fades(s: &mut Session, track: TrackId, range: Range, shape: FadeShape) -> usize {
    create_fades_with_shapes(s, track, range, shape, shape)
}

/// Fade-out and fade-in curves can be independently selected, as in the Pro Tools fade dialog.
pub fn create_fades_with_shapes(
    s: &mut Session, track: TrackId, range: Range, shape_out: FadeShape, shape_in: FadeShape,
) -> usize {
    if range.is_empty() {
        return 0;
    }
    // Resizing an existing crossfade must be one undoable edit. Restore the saved
    // original cut before recomputing handles, so previous extensions do not accumulate.
    let rebuild: Vec<(ClipId, ClipId, Samples)> = s.track(track).map_or_else(Vec::new, |tr| {
        tr.clips().windows(2).filter_map(|pair| {
            let (a, b) = (&pair[0], &pair[1]);
            let original = s.edit.values.get(&sculpt_xfade_join_key(a.id, b.id))?;
            let seam = *original as Samples;
            (range.start < seam && seam < range.end && a.end() > b.start && a.fade_out.len > 0 && b.fade_in.len > 0)
                .then_some((a.id, b.id, seam))
        }).collect()
    });
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        for (aid, bid, seam) in rebuild {
            let left = pl.clips.iter().position(|c| c.id == aid);
            let right = pl.clips.iter().position(|c| c.id == bid);
            if let (Some(i), Some(j)) = (left, right) {
                if i != j {
                    if let Some(a) = pl.clips.get_mut(i) {
                        a.fade_out = Fade::default();
                        a.trim_end_to(seam);
                    }
                    if let Some(b) = pl.clips.get_mut(j) {
                        b.fade_in = Fade::default();
                        b.trim_start_to(seam);
                    }
                }
            }
        }
    }
    let handles: Vec<(ClipId, Samples)> = {
        let Some(tr) = s.track(track) else { return 0 };
        tr.clips()
            .iter()
            .map(|c| {
                let avail = match c.content {
                    ClipContent::Audio { source, offset } => {
                        let total = s.source(source).map_or(0, |src| i64::try_from(src.frames).unwrap_or(i64::MAX));
                        total.saturating_sub(offset).saturating_sub(c.length)
                    }
                    ClipContent::Video { source, offset } => {
                        let total = s.video(source).map_or(0, |v| s.sample_rate.samples(v.duration.clamp(0.0, 1e7)));
                        total.saturating_sub(offset).saturating_sub(c.length)
                    }
                    ClipContent::Midi { .. } => 0,
                };
                (c.id, avail)
            })
            .collect()
    };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    pl.sort();
    let mut made = 0;
    let mut xfaded: Vec<ClipId> = Vec::new();
    let mut remembered_joins: Vec<(String, f64)> = Vec::new();
    let n = pl.clips.len();
    // Crossfades between adjacent clips whose boundary is inside the range.
    for i in 0..n.saturating_sub(1) {
        let (Some(a), Some(b)) = (pl.clips.get(i).cloned(), pl.clips.get(i + 1).cloned()) else { continue };
        let boundary = a.end();
        if b.start == boundary && range.start < boundary && range.end > boundary {
            let before = boundary - range.start.max(a.start);
            let after = range.end.min(b.end()) - boundary;
            let a_handle = handles.iter().find(|h| h.0 == a.id).map_or(0, |h| h.1);
            let b_handle = b.source_offset();
            // Extend each clip into the other's territory using its handles.
            let ext_a = after.min(a_handle).max(0);
            let ext_b = before.min(b_handle).max(0);
            if let Some(ca) = pl.clips.get_mut(i) {
                ca.length += ext_a;
                ca.fade_out = Fade { len: (ext_a + before).min(ca.length), shape: shape_out };
            }
            if let Some(cb) = pl.clips.get_mut(i + 1) {
                let old = cb.start;
                cb.trim_start_to(old - ext_b);
                cb.fade_in = Fade { len: (ext_b + after).min(cb.length), shape: shape_in };
            }
            made += 1;
            xfaded.push(a.id);
            xfaded.push(b.id);
            remembered_joins.push((sculpt_xfade_join_key(a.id, b.id), boundary as f64));
        }
    }
    for c in pl.clips.iter_mut().filter(|c| !xfaded.contains(&c.id)) {
        if range.start <= c.start && range.end > c.start && range.end < c.end() {
            c.fade_in = Fade { len: range.end - c.start, shape: shape_in };
            made += 1;
        } else if range.end >= c.end() && range.start > c.start && range.start < c.end() {
            c.fade_out = Fade { len: c.end() - range.start, shape: shape_out };
            made += 1;
        }
    }
    for (key, seam) in remembered_joins {
        s.edit.values.insert(key, seam);
    }
    made
}

/// Remove only fades intersecting the requested range (or the cursor position).
/// Crossfades restore the *original* edit boundary, not the middle of the overlap.
pub fn delete_fades(s: &mut Session, track: TrackId, range: Range) -> usize {
    let remembered = s.edit.values.clone();
    let mut used_keys = Vec::new();
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let selected = |start: Samples, end: Samples| -> bool {
        if end <= start { return false; }
        if range.is_empty() {
            start <= range.start && range.start < end
        } else {
            start < range.end && range.start < end
        }
    };
    let mut changed = 0usize;
    pl.sort();
    for i in 0..pl.clips.len().saturating_sub(1) {
        let (a, b) = (&pl.clips[i], &pl.clips[i + 1]);
        if a.end() <= b.start || a.fade_out.len <= 0 || b.fade_in.len <= 0 || !selected(b.start, a.end()) {
            continue;
        }
        let key = sculpt_xfade_join_key(a.id, b.id);
        let seam = remembered.get(&key).map_or(b.start + (a.end() - b.start) / 2, |x| *x as Samples);
        if seam <= a.start || seam >= b.end() { continue; }
        if let Some(left) = pl.clips.get_mut(i) {
            left.fade_out = Fade::default();
            left.trim_end_to(seam);
        }
        if let Some(right) = pl.clips.get_mut(i + 1) {
            right.fade_in = Fade::default();
            right.trim_start_to(seam);
        }
        used_keys.push(key);
        changed += 1;
    }
    for c in &mut pl.clips {
        let mut erased = false;
        if c.fade_in.len > 0 && selected(c.start, c.start + c.fade_in.len) {
            c.fade_in = Fade::default();
            erased = true;
        }
        if c.fade_out.len > 0 && selected(c.end() - c.fade_out.len, c.end()) {
            c.fade_out = Fade::default();
            erased = true;
        }
        if erased { changed += 1; }
    }
    for key in used_keys { s.edit.values.remove(&key); }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Source, SourceId, TrackKind};

    fn session_with_clip() -> (Session, TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.sources.push(Source {
            id: SourceId(500),
            name: "src".into(),
            path: "src.wav".into(),
            channels: 1,
            frames: 10_000,
            sample_rate: 48_000,
            format: soundcraft_audio_io::FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        let id = s.new_clip_id();
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.push(Clip::audio(id, "a", SourceId(500), 1000, 1000, 4000));
        }
        (s, t)
    }

    fn clips(s: &Session, t: TrackId) -> Vec<(Samples, Samples, Samples)> {
        s.track(t).unwrap().clips().iter().map(|c| (c.start, c.length, c.source_offset())).collect()
    }

    #[test]
    fn separate_splits_and_heal_rejoins() {
        let (mut s, t) = session_with_clip();
        assert_eq!(separate_at(&mut s, t, 3000), 1);
        assert_eq!(clips(&s, t), vec![(1000, 2000, 1000), (3000, 2000, 3000)]);
        assert_eq!(separate_at(&mut s, t, 1000), 0);
        assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 1);
        assert_eq!(clips(&s, t), vec![(1000, 4000, 1000)]);
    }

    #[test]
    fn clear_slip_and_shuffle() {
        let (mut s, t) = session_with_clip();
        let removed = clear_range(&mut s, t, Range::new(2000, 3000), false);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].start, 0);
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (3000, 2000, 3000)]);
        let (mut s, t) = session_with_clip();
        clear_range(&mut s, t, Range::new(2000, 3000), true);
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2000, 2000, 3000)]);
    }

    #[test]
    fn copy_paste_overwrites() {
        let (mut s, t) = session_with_clip();
        let copied = copy_range(&s, t, Range::new(1500, 2500));
        assert_eq!(copied.len(), 1);
        assert_eq!((copied[0].start, copied[0].length, copied[0].source_offset()), (0, 1000, 1500));
        paste(&mut s, t, 4000, &copied, 1000, false);
        assert_eq!(clips(&s, t), vec![(1000, 3000, 1000), (4000, 1000, 1500)]);
    }

    #[test]
    fn paste_shuffle_ripples() {
        let (mut s, t) = session_with_clip();
        let copied = copy_range(&s, t, Range::new(1000, 2000));
        paste(&mut s, t, 1000, &copied, 1000, true);
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2000, 4000, 1000)]);
    }

    #[test]
    fn insert_silence_and_trim() {
        let (mut s, t) = session_with_clip();
        insert_silence(&mut s, t, Range::new(2000, 2500));
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2500, 3000, 2000)]);
        let (mut s, t) = session_with_clip();
        trim_to(&mut s, t, Range::new(1500, 2000));
        assert_eq!(clips(&s, t), vec![(1500, 500, 1500)]);
    }

    #[test]
    fn sculpt_asymmetric_crossfade_restores_exact_junction() {
        let (mut s, t) = session_with_clip();
        separate_at(&mut s, t, 3000);
        assert_eq!(create_fades(&mut s, t, Range::new(2850, 3080), FadeShape::EqualPower), 1);
        let c = s.track(t).unwrap().clips();
        assert_eq!((c[0].end(), c[1].start), (3080, 2850));
        assert_eq!(delete_fades(&mut s, t, Range::new(2900, 3050)), 1);
        let c = s.track(t).unwrap().clips();
        assert_eq!((c[0].end(), c[1].start), (3000, 3000));
    }

    #[test]
    fn sculpt_repeated_crossfade_resize_does_not_accumulate_handles() {
        let (mut s, t) = session_with_clip();
        separate_at(&mut s, t, 3000);
        assert_eq!(create_fades(&mut s, t, Range::new(2900, 3100), FadeShape::EqualPower), 1);
        assert_eq!(create_fades(&mut s, t, Range::new(2800, 3050), FadeShape::SCurve), 1);
        let c = s.track(t).unwrap().clips();
        assert_eq!((c[0].end(), c[1].start), (3050, 2800));
        assert_eq!(delete_fades(&mut s, t, Range::new(2800, 3050)), 1);
        let c = s.track(t).unwrap().clips();
        assert_eq!((c[0].end(), c[1].start), (3000, 3000));
    }

    #[test]
    fn fades_and_crossfades() {
        let (mut s, t) = session_with_clip();
        assert_eq!(create_fades(&mut s, t, Range::new(500, 1500), FadeShape::Linear), 1);
        assert_eq!(s.track(t).unwrap().clips()[0].fade_in.len, 500);
        let (mut s, t) = session_with_clip();
        separate_at(&mut s, t, 3000);
        assert_eq!(create_fades(&mut s, t, Range::new(2900, 3100), FadeShape::EqualPower), 1);
        let c = s.track(t).unwrap().clips();
        assert_eq!(c[0].end(), 3100);
        assert_eq!(c[1].start, 2900);
        assert_eq!(c[0].fade_out.len, 200);
        assert_eq!(c[1].fade_in.len, 200);
        delete_fades(&mut s, t, Range::new(2900, 3100));
        let c = s.track(t).unwrap().clips();
        assert_eq!((c[0].end(), c[1].start), (3000, 3000));
    }

    #[test]
    fn moves_overwrite_and_respect_locks() {
        let (mut s, t) = session_with_clip();
        let id = s.track(t).unwrap().clips()[0].id;
        assert_eq!(move_clips(&mut s, &[id], 500, None), 1);
        assert_eq!(clips(&s, t), vec![(1500, 4000, 1000)]);
        s.find_clip_mut(id).unwrap().time_locked = true;
        assert_eq!(move_clips(&mut s, &[id], 500, None), 0);
    }
}
