# Core36 — confini delle scritture di automazione

23 ottobre 2026. Gate locale completo superato: 763 test numerici più harness UI, fmt/Clippy/assets/layers/wasm. Rust 1.99.1, macOS arm64; sandbox che esclude soltanto la lettura dei VST3 installati, fixture attive. Regressioni bridge con ricevitore Swift compilato dalla consegna SHOW `82355f8` senza modificare SHOW. Workspace formattato prima del gate, tre differenze preesistenti di formato escluse dal commit; warning wasm preesistente su CaptureStates. Nessuna nuova build universal o prova hardware.

Verificato nel codice il candidato B2 dell'audit [#9](https://github.com/mrgianluca990-bit/SculptDAW/issues/9). `AutomationLane::write_range` riscriveva il punto in `end+1` usando il valore calcolato in `end`, anche quando esisteva già un breakpoint dell'utente. Su una rampa di prova il punto -12 diventava -11.970076. La regressione è fallita sulla base precedente.

La scrittura conserva ora un breakpoint già presente nel campione successivo. Se manca, mantiene il punto di ritorno sintetico previsto dalla funzione. Usa inoltre checked_add per quel campione: quando end è i64::MAX non può aggiungere un anchor oltre il limite, né riscrivere il nuovo endpoint con il valore precedente. Anche questo caso è stato riprodotto prima della correzione.

Due nuove regressioni model verificano curva dopo il breakpoint, unicità dell'anchor, endpoint al massimo campione e range vuoto. Una regressione del comando reale automation.write_range verifica punti scritti, conservazione del breakpoint successivo e Undo/Redo con ripristino dello stato clean.

Nessuna modifica alla struttura serializzata, alle altre modalità di automazione, ai marker, al routing o a SHOW/bridge. Non è una misura della precisione temporale dell'intero playback: tale QA resta nel piano. Nessuna app intermedia prodotta; il programma completo rimane aperto.
