# Verifiche native e packaging esplicito

19 ottobre 2026. Supporto al piano completo, distinto dalle modifiche del motore audio.

La CI Core27 `39173098624` ha richiesto circa 75 minuti sul runner Intel: controlli ripetuti per sottoinsiemi, gate completo e successiva build universal. Il piano prevede molti piccoli interventi; produrre un installer dopo ciascuno consumerebbe risorse anche quando l'utente ha richiesto un'app soltanto alla fine.

Il workflow corrente esegue un solo `cargo xtask ci` per PR: fmt, Clippy, tutti i test workspace (unitari e integrazione), assets, layers e wasm. Il validatore Swift originale resta compilato e disponibile nei test del contratto. Non sono eliminati i test VST3 fixture, I/O, playback o bridge: erano sottoinsiemi già inclusi nel gate completo.

`macos-15` è un runner Apple Silicon secondo la [documentazione ufficiale GitHub](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). Due compilazioni Rust concorrenti limitano il picco di memoria sul runner; questo valore non è il numero di worker audio della DAW. I tempi del nuovo flusso vanno misurati sulle run effettive, senza promettere un miglioramento percentuale.

- PR verso `main`, `develop`, `feature/**`: gate completo nativo, log conservato come artifact. Nessuna build universal automatica per ogni piccolo intervento.
- Push su `main`/`develop`: gate completo.
- Push su `release/**`: gate completo, anteprime, app universal arm64+x86_64 con CLI incorporata, checksum/CRC e artifact.
- Avvio manuale: `package_archive=true` (default) produce la stessa distribuzione dopo il gate; `false` esegue soltanto le verifiche.

Il packaging rimane quello esistente, incluse firma e verifica del bundle. Il log del gate è conservato anche in caso di fallimento; non si carica un'app se il gate fallisce. La formattazione preventiva è ereditata e ancora dichiarata: la pulizia delle tre differenze di formato della base sarà un intervento distinto. Nessuna modifica ai sorgenti SHOW o bridge.

Validazione locale: YAML letto dal parser, condizioni di packaging e copertura dei sottoinsiemi confrontate con `xtask::ci`; esecuzione GitHub da verificare sulla PR. Il gate locale completo viene riportato nella PR con le condizioni effettive di esecuzione.
