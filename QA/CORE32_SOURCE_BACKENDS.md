# Core32 — sorgenti, mixer e trasporto su disco

19 ottobre 2026. Revisione verificata dei componenti. Non ancora abilitata nei caricamenti dell'app: import/open/take restano residenti finché tutti i consumatori sono adattati. Nessun nuovo ZIP finale; il piano completo resta attivo.

## Implementazione

`SourceAudio` rappresenta PCM residente oppure WAV registrato nella cache, con metadati e waveform limitate. Frames/channels/sample_rate/has_uniform_channels non eseguono I/O. Le sorgenti disco mantengono soltanto il rate in `buffer` e nessun vettore PCM: i consumatori devono usare letture backend-aware. `channel()` serve esclusivamente ai residenti.

`read_frames`, `resident_copy` e `SourcePool::resident_snapshot` sono API offline/controllo. La copia richiede budget esplicito, verifica la somma prima di iniziare, conserva gli ID e condivide gli Arc già residenti. Non muta la sorgente, le waveform o Undo. `StreamSource::open_reader` verifica stamp e header.

Il mixer riceve `PreparedMedia` costruito sul controllo: una view indipendente per clip, geometria offset/stretch, prefetch delle clip in arrivo, destinazione loop e rilascio delle richieste lontane. I cambi a mixer/automazione riusano le view quando clip e identità media sono immutate. Le richieste di più view vengono raggruppate mediante un epoch atomico del pool: il worker non può dichiarare capienza insufficiente su un insieme aggiornato soltanto a metà. Il render fissa pagine senza file I/O; se manca audio in-file annulla l'intero blocco di output, senza inviare un mix parziale al dispositivo, e registra l'errore.

Il bounce `render_range` prepara le finestre prima di ogni blocco sul controllo; non anticipa clip estranee fuori dal range richiesto. Il player prepara Play/Locate e gli aggiornamenti di sessione sul controllo, consegna sessione/media insieme al callback e ferma il trasporto sugli errori. I proprietari di sessioni e mappe ritirate sono mantenuti dal controllo fino al rilascio nel callback. La UI mostra il problema e finalizza REC tramite il normale Stop. Gli errori di preparazione non fanno partire il trasporto né segnano come attivi plugin mai consegnati. STOP elimina i campioni pending prima del monitoraggio input. Il fallback silent clock conserva il documento corrente.

## Verifiche mirate già passate

- Tre test sorgenti: metadati/range/copie esatti su 7 canali, budget totale/ID/Arc nello snapshot offline, limiti/cancellazione/media modificati.
- Un test batch con scope annidati: una sovracapienza transitoria non resta associata alle view, poi la richiesta coerente si prepara.
- Cinque test mixer: bounce disco/residente identico con mono/stereo/7ch, due clip sovrapposte e posizioni indipendenti, offset/stretch/fade; errore esplicito e nessuna lettura disco durante render non preparato; loop e clip in arrivo pronti; media cambiati rifiutati; file fuori dal bounce non letti.
- Quattro test player senza dispositivi reali: Play/loop/Locate, riuso delle view dopo cambio mixer, stop/notifica su worker arrestato, preflight fallito senza avanzamento, rilascio delle mappe ritirate sul controllo, eliminazione del pending prima di input monitoring.
- Gate workspace completo locale passato: 746 test con riepilogo numerico più harness UI; fmt, Clippy, assets, layers e wasm. Rust 1.99.1 su macOS arm64, workspace formattato prima del gate; le tre differenze di formato preesistenti sono escluse dal commit. Sandbox che esclude dalla lettura soltanto i VST3 installati, fixture attive. Test bridge con validatore Swift compilato dalla copia SHOW `9eb1a57`, senza modificarla. Resta il warning wasm preesistente su CaptureStates. Nessuna nuova build universal in questa fase.

Non è una misura di assenza di allocazioni nel callback dell'intera DAW: restano preparazione del grafo, Rayon, canali di comando/garbage, misure di deadline/RSS e ascolto hardware da completare nel piano. La preparazione sincrona può impegnare il controllo/UI; timeout esplicito di tre secondi. I limiti del pool e delle view restano quelli Core30.

## Prima dell'attivazione nei caricamenti

1. Adattare render_bus, render_clips/consolidate e render_track_pre_fader/commit, sampler, time-stretch, health, save/copy, bundle e clip-group. Nessun consumatore deve usare buffer vuoti come se fossero PCM pronto.
2. Bridge congelato: `cmd/sculpt_bridge.rs` controlla buffer.channels e richiede residenti. Non modificare file o protocollo. Servono uno snapshot temporaneo compatibile con le API offline e regressioni su documento/revisione/dirty/Undo/ID, anche su errore/panic, prima di abilitare sorgenti disco.
3. Collegare import/open/recordings a un pool condiviso per progetto, conservare gli originali e convertire offline quando necessario. Budget waveform complessivo fra sorgenti, non soltanto fra canali.
4. Misurare PCM/RSS con file lunghi, seek, loop, molte sorgenti e take dopo STOP. Collaudare le API consumer reali, bridge con ricevitore Swift e audio prima di dichiarare risolto il caricamento integrale.

SHOW e bridge invariati. Le nuove factory disco sono per ora attivate soltanto dalle regressioni.
