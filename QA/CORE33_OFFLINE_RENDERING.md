# Core33 — consumatori offline delle sorgenti su disco

19 ottobre 2026. Revisione verificata. Gate completo locale superato con 752 test numerici più harness UI, fmt, Clippy, assets, layers e wasm. Rust 1.99.1 su macOS arm64; processo di test in sandbox che esclude soltanto la lettura dei VST3 installati, fixture attive. Validatore Swift compilato dalla copia SHOW `9eb1a57` nelle regressioni bridge. Tre file con formattazione preesistente esclusi dal commit, dopo gate su workspace formattato. Warning wasm preesistente su CaptureStates. Nessuna nuova build universal o prova hardware in questa fase.

Bounce dei bus, consolidamento e commit/freeze dispongono di API checked che restituiscono errori espliciti. I comandi di bounce, editing distruttivo, transient detection, Strip Silence e AudioSuite propagano questi errori, invece di trattare un risultato vuoto come silenzio utile. I wrapper legacy conservano la firma Vec, registrano un warning e restituiscono un risultato vuoto su errore; i percorsi di produzione dell'engine usano le varianti checked.

Il consolidamento legge il PCM della sorgente a blocchi senza materializzarla. Ogni clip inizia dal primo campione di sovrapposizione, conservando la fase dell'interpolazione del gain a 32 campioni della funzione residente. La successiva elaborazione Clip FX mantiene il suo pre-roll di zeri e la segmentazione precedente. Il render dei bus prepara le pagine fuori dal render e controlla media e plugin prima di accettare il risultato. Il commit riusa il render checked isolato della traccia e ora conserva la larghezza multicanale anche nel nuovo formato traccia: prima veniva limitata a stereo.

Le API checked rifiutano richieste oltre il limite offline esistente, compreso il padding di latenza, prima dell'allocazione del risultato. Rifiutano anche sorgenti mancanti, vuote, ragged o a frequenza incoerente con il progetto. I limiti dei campioni fuori dai bordi della clip mantengono il comportamento precedente; un errore in-file non viene sostituito con zeri. Questa fase non è una conversione SRC e non cambia il codice o protocollo bridge.

Il risultato del render resta un Vec limitato dal budget offline già esistente (2 GiB di campioni; FX e altri scratch sono ulteriori): questo intervento elimina la necessità di caricare il PCM dell'intera sorgente per questi consumatori, non rende costante la RAM di un bounce arbitrariamente lungo. Registrazione/caricamento, writer di export progressivo e misure RSS restano nel piano completo.

## Prove aggiunte

- Render disco/residente identico per mono/stereo/7ch, anche con range e fade non allineati, envelope gain, clip FX, commit e bounce bus.
- File modificati/troncati e richieste sopra budget rifiutati senza restituire audio parziale né iniziare una lettura per richieste fuori limite.
- PCM con rate errato, canali di durata diversa o zero frame rifiutato da tutti i render checked.
- Tab to Transient su audio non disponibile segnala il problema senza muovere selezione o playhead.
- Comandi su media cambiati e su pool mancante restituiscono EngineError::Io; documento, stato dirty e Undo restano invariati per Strip Silence, Consolidate, Separate at Transients, Commit e Audio as MIDI.
- Consolidamento/commit veri dell'engine da sorgenti disco identici a quelli residenti, compreso Undo; commit 7ch conserva sette canali nella nuova traccia e sorgente. Gli originali restano su disco.

## Ancora necessari prima di attivare import/open/take su disco

Salvataggi/copie/bundle/clip-group, time-stretch, sampler, health e compatibilità offline tramite snapshot per il renderer bridge congelato. Quindi pool per progetto, conversioni offline conservando gli originali, caricamento take e misure di memoria/tempo. SHOW e file bridge invariati. Nessuna app finale prodotta; nessuna chiusura del piano completo.
