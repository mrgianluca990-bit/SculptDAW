# Core35 — normalizzazione con fattori estremi

23 ottobre 2026. Gate completo locale superato: 760 test numerici più harness UI, fmt/Clippy/assets/layers/wasm. Rust 1.99.1 su macOS arm64; test in sandbox che esclude soltanto i VST3 installati, fixture attive. Workspace formattato prima del gate; tre differenze di formato preesistenti escluse dal commit e warning wasm preesistente su CaptureStates. Nessuna nuova build universal o prova hardware.

Il nuovo audit [#9](https://github.com/mrgianluca990-bit/SculptDAW/issues/9) segnala il fix upstream per picchi subnormal. Verificati il sorgente Sculpt effettivo e la release primaria [SoundCraft v0.12.1](https://github.com/storytold/soundcraft/releases/tag/v0.12.1), pubblicata il 22 ottobre; riferimento immutabile [offline.rs e85a480](https://github.com/storytold/soundcraft/blob/e85a4801e9029c9a1f5ce536b11be65aea832ed8/crates/dsp/src/offline.rs). Non è stato effettuato alcun merge upstream.

Prima, `target / peak` in f32 poteva diventare infinito per un picco subnormal: anche campioni originariamente finiti, inclusi gli zeri, diventavano Inf/NaN. La regressione subnormal è fallita sulla base non corretta. Esiste inoltre il caso opposto: un picco f32 molto grande e un target non nullo basso possono produrre gain subnormal/zero, perdendo il livello richiesto. La nuova regressione verifica esplicitamente il vecchio calcolo che diventa zero a -140 dBFS. Il limite -144 dB rimane intenzionalmente silenzio, secondo il contratto di db_to_gain.

I fattori ordinari usano esattamente il percorso f32 precedente. Solo gain non normali, con target non nullo, vengono calcolati e applicati in f64 prima di tornare a f32. Ciò copre overflow e perdita di precisione/underflow. Restano le modalità linked/per-channel e il comportamento dei canali silenziosi; nessuna nuova allocazione nella funzione. Nessuna modifica a LUFS, pan, routing, codec, renderer/protocollo bridge o SHOW.

Tre regressioni DSP coprono subnormal mono/stereo e zeri, picco massimo e target basso, normale rapporto fra canali, uguaglianza del percorso ordinario, buffer vuoti, zero con segno e floor. Una regressione dell'engine esegue AudioSuite Normalize su una clip subnormal: risultato finito, WAV float32 coerente, sorgente originale conservata e Undo/Redo corretti.

Per il gate è stato compilato, in /tmp da git archive in sola lettura, il ricevitore Swift della consegna SHOW `82355f80b187c6ab37a449dad4b13f24db88c2e2` (29 sorgenti Swift Core e tre C). Le prove precedenti sul ricevitore `9eb1a57` restano baseline documentata; il gate passato aggiunge evidenza di compatibilità con la consegna più recente indicata dall'utente. Il repository SHOW non viene modificato e non si dichiara prova di ascolto/round-trip UI reale.

Il piano completo resta attivo: storage e sampler prima di abilitare i loader disco, poi le altre fasi del documento utente. Nessuna nuova app intermedia prodotta.
