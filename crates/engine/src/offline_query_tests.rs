use super::*;
use soundcraft_audio_io::{
    AudioBuffer, BitDepth, EncodeOptions,
    stream_cache::{CacheConfig, StreamCache},
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sculpt-offline-query-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn engines(fixture: &Fixture, cache: &Arc<StreamCache>) -> (Engine, Engine) {
    let mut resident = Engine::default();
    for channels in [1, 2] {
        let buffer = AudioBuffer {
            sample_rate: 48_000,
            channels: (0..channels).map(|ch| (0..6000).map(|f| (f as f32 * 0.01 + ch as f32).sin() * 1.5).collect()).collect(),
        };
        let bytes = soundcraft_audio_io::encode(&buffer, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
        crate::io::import_audio_bytes(&mut resident, "Same", &bytes, None, None, 0).unwrap();
    }
    resident.execute("markers.add", &json!({"name":"IN", "at":0})).unwrap();
    resident.execute("markers.add", &json!({"name":"OUT", "at":4800})).unwrap();
    let mut disk = Engine::new(resident.session().clone());
    for source in disk.session().sources.clone() {
        let path = fixture.0.join(format!("source-{}.wav", source.id.0));
        let audio = disk.session().pool.get(source.id).unwrap();
        std::fs::write(
            &path,
            soundcraft_audio_io::encode(&audio.buffer, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap(),
        )
        .unwrap();
        let audio = model::SourceAudio::from_stream(cache.open_source(path).unwrap(), 140, |_, _| true).unwrap();
        disk.session_mut().pool.insert(source.id, Arc::new(audio));
    }
    disk.execute("mix.volume", &json!({"track": disk.session().tracks[0].id.0, "db": -3.0})).unwrap();
    resident.execute("mix.volume", &json!({"track": resident.session().tracks[0].id.0, "db": -3.0})).unwrap();
    (resident, disk)
}

#[test]
fn disk_query_uses_unchanged_renderer_and_preserves_document_history_and_stem_order() {
    let fixture = Fixture::new();
    let cache = StreamCache::new(CacheConfig::default()).unwrap();
    let (mut resident, mut disk) = engines(&fixture, &cache);
    let before = disk.session_arc();
    let revision = disk.revision;
    let dirty = disk.is_dirty();
    let history = disk.undo_history();
    let journal = disk.journal.clone();
    let result = disk.execute("bridge.render_stems", &json!({"dir":fixture.0.join("disk")})).unwrap();
    let expected = resident.execute("bridge.render_stems", &json!({"dir":fixture.0.join("resident")})).unwrap();
    let actual = result["files"].as_array().unwrap();
    let expected = expected["files"].as_array().unwrap();
    assert_eq!(actual.len(), 2);
    for (index, (a, b)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(a["trackID"], b["trackID"]);
        assert_eq!(a["trackID"], disk.session().tracks[index].id.0);
        assert_eq!(a["channels"], b["channels"]);
        assert_eq!(std::fs::read(a["path"].as_str().unwrap()).unwrap(), std::fs::read(b["path"].as_str().unwrap()).unwrap());
    }
    assert!(Arc::ptr_eq(&before, &disk.session_arc()));
    assert_eq!(disk.revision, revision);
    assert_eq!(disk.is_dirty(), dirty);
    assert_eq!(disk.undo_history(), history);
    assert_eq!(disk.journal, journal);
    assert!(disk.session().sources.iter().all(|s| !disk.session().pool.get(s.id).unwrap().is_resident()));
    assert!(disk.execute("bridge.render_stems", &json!({"dir":fixture.0.join("disk")})).is_err());
    assert!(Arc::ptr_eq(&before, &disk.session_arc()));
    assert_eq!(disk.session().markers, before.markers);
    let source = disk.session().sources[0].id;
    std::fs::OpenOptions::new().write(true).open(fixture.0.join(format!("source-{}.wav", source.0))).unwrap().set_len(100).unwrap();
    let failed_dir = fixture.0.join("failed");
    assert!(disk.execute("bridge.render_stems", &json!({"dir":failed_dir})).is_err());
    assert!(!failed_dir.exists());
    assert!(Arc::ptr_eq(&before, &disk.session_arc()));
    assert_eq!(disk.revision, revision);
    assert_eq!(disk.undo_history(), history);
    cache.shutdown(Duration::from_secs(1)).unwrap();
}

#[test]
#[allow(clippy::panic)]
fn isolated_query_budget_and_panic_cannot_modify_the_open_engine() {
    let fixture = Fixture::new();
    let cache = StreamCache::new(CacheConfig::default()).unwrap();
    let (_, disk) = engines(&fixture, &cache);
    let spec = CommandSpec {
        id: "test.offline",
        label: "Test",
        menu: &[],
        shortcut: None,
        params: "",
        enabled: |_| Ok(()),
        run: |temporary, _| {
            temporary.execute("track.new", &json!({})).unwrap();
            panic!("injected offline query panic")
        },
        journal: false,
        undoable: false,
        view: false,
    };
    let before = disk.session_arc();
    let revision = disk.revision;
    let history = disk.undo_history();
    let ids: Vec<_> = disk.session().pool.ids().collect();
    assert!(matches!(disk.execute_resident_query(&spec, &json!({}), ids.clone(), 1), Err(EngineError::Io(_))));
    assert!(matches!(disk.execute_resident_query(&spec, &json!({}), ids, 18_000), Err(EngineError::Internal(_, _))));
    assert!(Arc::ptr_eq(&before, &disk.session_arc()));
    assert_eq!(disk.revision, revision);
    assert_eq!(disk.undo_history(), history);
    cache.shutdown(Duration::from_secs(1)).unwrap();
}
