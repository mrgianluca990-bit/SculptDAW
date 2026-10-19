# Core30 — pool PCM condiviso e prefetch

19 ottobre 2026. Revisione iniziale. Componente nativo `audio-io::stream_cache`, non ancora collegato al player. Nessuna modifica a SHOW o al bridge. Dipende dal lettore Core29.

Un solo worker disco serve sorgenti e posizioni indipendenti. Il pool contiene 32 MiB di PCM per default, configurabili fino a 256 MiB, indipendentemente dalla durata dei file; il budget non include header, strutture delle view, scratch di decodifica e stack. Le pagine hanno un numero fisso di campioni: i canali riducono i frame per pagina, senza moltiplicare il budget. Al massimo 4096 view e 64 file aperti nel worker. Vecchie sorgenti conservate da Undo condividono il pool.

Il render richiede finestre e fissa le pagine mediante atomiche. Durante la lettura una pagina non può essere riutilizzata dal worker. Mancanza di pagine, errore disco, media cambiati e arresto restituiscono errori espliciti; non una finestra parzialmente riempita di zeri. Il contatore cache_misses misura acquisizioni di finestre non pronte, non dropout fisici del dispositivo. I guasti restano associati alla view; il retry esplicito crea una view nuova.

Prepare attende sia la finestra primaria sia l'eventuale destinazione loop. Il worker ammette prima le pagine necessarie, poi il lookahead entro il budget residuo. Una pagina obsoleta ancora fissata può ritardare la preparazione, senza generare falsi errori di capienza. Registrazione, preparazione bloccante, messaggi di errore e shutdown appartengono al thread di controllo. Il proprietario del pool resta fuori callback. Shutdown ha un timeout: non rende cancellabile una lettura OS bloccata.

## Verifiche del componente

Sette regressioni: campioni float esatti mono/stereo/7ch attraversando pagine e seek; anchor disponibile dopo Prepare; PCM limitato e byte letti inferiori al file; pagine vecchie fissate durante il riuso; capienza insufficiente e retry esplicito; media modificati e troncati; sorgenti e view indipendenti; due thread con posizioni concorrenti e ripetute sostituzioni; configurazioni/range fuori limite e fine file vuota. Gate locale finale superato: 727 test con riepilogo numerico, oltre alle prove UI con harness; fmt, Clippy, assets, layers e wasm passati. Rust 1.99.1 su macOS arm64; sandbox che esclude soltanto la lettura dei VST3 installati, fixture attive. Validatore Swift SHOW della snapshot `9eb1a57` usato nelle regressioni bridge, senza modificare SHOW. Il check wasm conserva il warning preesistente su CaptureStates.

La separazione disco/callback è proprietà delle API e del codice di questo componente. Non è ancora una misura realtime della DAW: integrazione del mixer/player, policy underrun, preparazione delle clip, waveform limitate e salvataggi/offline restano necessari. Nessun nuovo ZIP intermedio prodotto.
