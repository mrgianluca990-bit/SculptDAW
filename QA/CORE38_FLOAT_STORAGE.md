# Core38 — headroom dei file float

23 ottobre 2026. Riprodotta perdita di dati nel codec precedente: il campione WAV float 1,5 veniva scritto come 1. La regressione bit-per-bit è fallita sulla base Core37.

WAV float32 e AIFF-C fl32 conservano ora tutti i valori finiti, inclusi headroom sopra 0 dBFS, subnormal e zero con segno. NaN e infiniti vengono scritti come silenzio, coerentemente con la decodifica float. La quantizzazione intera conserva clipping e dither precedenti. Nessuna modifica ai limiti hardware del player.

Prove: round-trip bit-per-bit su due canali per WAV/AIFF; policy non-finiti e clipping intero; session save/reopen, save copy e bundle con campioni 1,5; render stem mono a +6,0206 dB con sorgente 0,75, conservazione del documento e stato dirty. L’atteso mono include la legge pan esistente -3 dB e la compensazione SQRT_2 del bridge (circa 1,50178): non si cambia quella politica per far coincidere il test con 1,5. La regressione storica che richiedeva clipping dei float è aggiornata alla policy esplicita; le sue asserzioni sui formati interi restano.

Il codice bridge e SHOW non sono modificati. Questo cambiamento rende il contenuto del file float fedele al render esistente, senza alterare schema, ordine stem o routing. Headroom nei file non equivale a maggiore headroom del DAC: resta necessario il collaudo dei livelli reali. Nessuna consegna finale. Gate completo locale passato: 776 test numerici più harness UI, fmt/Clippy/assets/layers/wasm; sandbox che esclude soltanto la lettura dei VST3 installati e ricevitore SHOW consegnato 82355f8. Non prova hardware/plugin reali.
