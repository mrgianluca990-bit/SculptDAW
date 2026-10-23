# Core41 — campioni degli strumenti preparati fuori callback

23 ottobre 2026. In verifica; loader disco non ancora attivo.

PreparedMedia prepara i file disco assegnati agli strumenti attivi come copie PCM residenti, fuori callback, con budget cumulativo di 64 MiB per sorgenti disco nella snapshot corrente. Strumenti sullo stesso source ID condividono la copia. Campioni già residenti sono condivisi senza duplicare PCM. File mancanti, vuoti, non validi o oltre budget rifiutano la preparazione; non si caricano automaticamente file arbitrariamente grandi dentro il callback.

Il player riusa le copie quando si ricreano le view per edit di clip e mantiene le media immutate per soli edit mixer. Identità della sorgente cambiata richiede nuova preparazione. sync_sample usa soltanto riferimenti preparati; un MixEngine chiamato senza preparazione di una sorgente disco segnala NotReady e azzera il blocco secondo il percorso errori media esistente. Tracce inattive non richiedono preparazione.

Corretto anche il retry della preparazione: non cancella il latch prima di una ricostruzione riuscita. Una preparazione fallita deve restare errore su PLAY ripetuti; altrimenti il secondo tentativo poteva utilizzare la media vuota precedente. Se update_session rifiuta una nuova snapshot, un latch impedisce di riprendere la vecchia sessione: solo un aggiornamento valido lo cancella. Due regressioni player ripetono PLAY tre volte e verificano lo sblocco dopo un aggiornamento valido.

Prove mirate: copia condivisa/riuso e consegna al sampler, budget/missing/mutazione/inactive; render MIDI del sampler da disco identico bit-per-bit al residente e non silenzioso, errore esplicito senza preparazione; PLAY ripetuti su campione mancante. Il primo gate ha individuato un’aspettativa storica di silenzio per media sampler mancanti: regressione aggiornata al rifiuto esplicito del render nativo con documento invariato; il browser conserva il comportamento precedente. Gate completo locale passato: 791 test numerici più harness UI, fmt/Clippy/assets/layers/wasm, sandbox con sola esclusione lettura VST3 installati e ricevitore SHOW 82355f8 invariato. Collaudo plugin/hardware reale distinto.

Limiti: il sampler attuale richiede slice PCM complete, quindi usa questa preparazione bounded, non un nuovo sampler streaming. Superare il budget richiede consolidare/accorciare il campione o una successiva architettura streaming dello strumento. Cache audio timeline separata. Nessuna modifica SHOW/bridge, nessuna nuova eliminazione SRC generale, nessuna consegna finale.
