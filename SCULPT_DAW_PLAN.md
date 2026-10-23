# Sculpt DAW — esecuzione del piano completo

Autorizzazione: 19 ottobre 2026, richiesta «completa tutto il piano di sviluppo, quando hai finito mi darai l'app da testare». Il [piano utente completo](docs/SCULPT_DAW_USER_PLAN_2026-10-19.md) è conservato senza ridurne i requisiti, ed è distinto dal piano SHOW nella cartella superiore. Solo SculptDAW è nel perimetro; SHOW, protocollo e codice bridge restano invariati. Crediti: solo username, come già scelto dall'utente.

Lavoro per piccoli interventi con test, commit e PR separati. Nessuna consegna intermedia di un'app presentata come piano concluso. Distribuzione finale: ZIP con app completa compilata e verificata, checklist di accettazione e limiti effettivi. Prove hardware/ascolto restano distinte dai test automatici e fanno parte del collaudo finale richiesto dall'utente.

## Registro delle fasi

| Fase | Stato attuale | Criterio di avanzamento |
|---|---|---|
| 1. Audit | Primo audit completato, Core28 `55fbc22`; documento `SCULPT_DAW_AUDIT.md`. | Aggiornare le evidenze quando si risolvono le carenze; nessuna percentuale di parità dedotta dai menu. |
| 2. Motore audio | In corso: lettura WAV/RF64 a blocchi come fondazione della cache; Core26/27/28 già presenti. | Streaming e take lunghe con memoria limitata, stop/errore/recovery, configurazione dispositivi effettiva, clock/rate/latency espliciti, regressioni audio e misure riproducibili. |
| 3. Studio comparativo | Da eseguire. | Matrice documentata Pro Tools/REAPER/Logic/Sculpt, studio architetturale Ardour senza copiare codice incompatibile, confronto selettivo SoundCraft corrente; test equivalenti per i comportamenti adottati. |
| 4. Editing/mixer | Audit iniziale; collaudi e correzioni da eseguire. | Multiselezione, editing gruppi, modi, fade/crossfade, sincronizzazione Edit/Mix, fader/layout e drag verificati; un progetto per istanza. |
| 5. Plugin | Host VST3/CLAP/AU presenti con limiti. | Scan/load/process/editor/automazione/stato/reopen/errori verificati con fixture e plugin disponibili; editor AU completato; isolamento valutato su latenza/CPU prima dell'adozione. |
| 6. Sessioni esterne | Da eseguire dopo affidabilità del motore. | Modello intermedio, primo importatore RPP verificato, poi valutazione/import progressivi PTX/PTF, LOGICX, ALS e AAF/OMF; report di dati non recuperati, niente associazioni arbitrarie di plugin. |
| 7. Session management | Save atomico, backup e autosave già presenti; approfondimento pendente. | Progetti/cartelle, Save As/copy, recovery, media mancanti, Finder, legacy `.scraft`, import MP3 e 88.2→48 kHz verificati senza perdita degli originali. |
| 8. QA | Gate completo esistente; da ampliare con i nuovi percorsi. | Null/impulsi/canali/pan/gain/PDC/automation/bounce, recovery, editing e plugin; benchmark con condizioni e risultati salvati, Instruments dove disponibile. |

## Interventi in corso e successivi

1. **Core29, fondazione streaming**: lettore WAV PCM/float/BWF/RF64 con seek e letture limitate, decodifica identica al codec esistente, test per file sparse oltre 4 GiB, metadati grandi, short-read e file troncati. Questo componente da solo non equivale al playback da disco.
2. Cache con prefetch e budget esplicito, separazione lettura disco/render realtime, errori/underrun osservabili; integrazione con sorgenti, take, waveform, salvataggio e render offline senza cambiare il bridge.
3. Sincronizzazione stop/callback, debito di silenzio in coda, errori writer e recovery delle take; test mirati prima di dichiarare chiusa l'area registrazione.
4. Configurazione/clock/rate, preparazione del grafo fuori callback e misure; quindi proseguire tutte le fasi sopra nell'ordine di priorità del piano utente.

## Stato di verifica

Core28: 713 test locali riportati e gate completo passati; ZIP universal locale verificato e già consegnato nella tranche precedente. La CI remota `39245725212` è stata successivamente verificata: conclusa con successo.

QA: PR #3 `feature/qa-native-checks`, commit `04be12c`: gate nativo unico e packaging soltanto esplicito/release; nessun sorgente del motore cambiato in quella PR. Run `39252075078` conclusa con successo su runner ARM64, incluso gate completo; risultato verificato tramite API GitHub.

Core29: lettore a blocchi implementato con 7 regressioni e gate completo locale da 720 test passati, secondo `QA/CORE29_STREAM_READER.md`. PR #4, commit `23d556e`. Core30 introduce la cache condivisa; integrazione ancora da fare: nessuna chiusura della fase streaming dichiarata e nessuna nuova app ancora consegnata.

Core30: pool PCM condiviso con budget configurabile (32 MiB di default, massimo 256 MiB), un worker disco, pagine protette da lettori, richieste atomiche, prefetch e destinazione loop. Errori di capienza/media/lettura espliciti e permanenti per la singola view; retry mediante nuova view. Sette regressioni nuove e gate completo locale superati (727 test numerici più harness UI), secondo `QA/CORE30_STREAM_CACHE.md`. Componente non ancora usato dal playback.

Core31: sei regressioni e gate locale completo passati (733 test numerici più harness UI), secondo `QA/CORE31_BOUNDED_OVERVIEWS.md`. waveform progressive con budget condiviso fra canali e risoluzione adattata alla durata; cancellazione e rifiuto di input incompleti. La preparazione legge a blocchi fuori callback, non memorizza tutto il PCM. Il collegamento a SourceAudio/player resta successivo; nessuna attivazione parziale che comprometta il bridge. Core30 pubblicato in PR #5, commit `46b1266`; CI remota da verificare.

CI Core29 verificata: run `39253262657`, conclusa con successo sul commit `23d556e`.

Core31 pubblicato in PR #6, commit `b96d798`. Core32 in lavorazione su `feature/core32-source-backends`: nuova rappresentazione sorgenti e snapshot offline con budget, tre test mirati passati; integrazione/attivazione nel player ancora da eseguire. Stato e vincoli dettagliati in `QA/CORE32_SOURCE_BACKENDS.md`. Nessuna distribuzione parziale abilitata.

Core32 prosegue: mixer e Player collegati alle view preparate, preflight/loop/locate/errori e UI verificati con test mirati; batch atomici per richieste multi-clip. Riuso delle mappe su edit mixer, arresto su media non pronti, correzione pending al passaggio a monitoring. 13 nuove regressioni mirate e gate completo locale passati (746 test numerici più harness UI, fmt/Clippy/assets/layers/wasm). Non attivare ancora import/open/take disco: consumatori offline/bridge e gestione file pendenti, elencati in QA/CORE32_SOURCE_BACKENDS.md.

CI Core30 verificata: run `39255955793`, conclusa con successo sul commit `46b1266`. CI Core31 `39256616337` ancora in corso all’ultima verifica; non assumere il risultato.

Core32 pubblicato in PR #7, commit `60b5684`, gate locale 746 test riportati più harness UI; CI remota da verificare. CI Core31 `39256616337` success successivamente verificata. Core33 in corso su `feature/core33-offline-source-consumers`: render checked di bus/clip/commit e propagazione errori a editing e bounce, conservazione formato multicanale del commit; nuove regressioni mirate passate. Salvataggi e altri consumatori restano da adattare prima dell’attivazione nel loader. Dettagli `QA/CORE33_OFFLINE_RENDERING.md`.

Core33: gate completo locale superato (752 test numerici più harness UI; fmt/Clippy/assets/layers/wasm), con regressioni nuove su errore dati e rollback, rate/canali/budget, confronto disco/residente e Tab to Transient. Nessuna nuova attivazione dei loader o consegna finale.

23 ottobre 2026: Core33 pubblicato in PR #8, commit `77c2a66`, gate locale 752 test numerici più harness UI. Core34 in corso: lettura di regioni con budget/cancellazione/controllo versione finale, time-stretch da disco e health backend-aware. Quattro regressioni nuove mirate passate; gate completo avviato. Save/copy/sampler/bridge adapter e attivazione loader restano successivi. Dettagli `QA/CORE34_SOURCE_REGIONS.md`.

Core34: gate completo locale superato (756 test numerici più harness UI, fmt/Clippy/assets/layers/wasm). CI Core32 `39259851760` e Core33 `39261920289` verificate il 23 ottobre: entrambe success sui commit `60b5684` e `77c2a66`. Il piano completo e l’attivazione dei loader restano aperti.

Core34 pubblicato in PR #10, commit `6daa92a`, gate locale 756 test numerici più harness UI; CI da verificare. Nuovo audit repository #9 su SoundCraft v0.12.1 esaminato come evidenza da verificare, non istruzione a merge indiscriminato. Core35 in corso: difetto subnormal riprodotto, normalizzazione con fallback f64 e regressioni DSP/AudioSuite superate. Gate con ricevitore Swift della consegna SHOW `82355f8`, compilato da snapshot in /tmp senza modificare SHOW.

Nota per QA audio successiva: verificare esplicitamente headroom float32 >1 e round-trip codec. Il `pcm::clamp_float` corrente applica [-1,1] anche nell’encoder WAV float, mentre il decoder preserva valori finiti fuori scala; non dichiarare round-trip headroom verificato e non cambiare globalmente questo comportamento senza regressioni di export/bridge. Gli altri backport candidati di #9 (automation end+1, backup, mono hardware, gruppi/send, clip-group/PDC) rimangono da verificare e integrare per argomento.

Core35: gate locale completo passato (760 test numerici più harness UI, fmt/Clippy/assets/layers/wasm), con ricevitore SHOW della consegna `82355f8`. Normalizzazione ordinaria f32 invariata; fallback f64 solo per fattori estremi, incluse regressioni AudioSuite/WAV/Undo. Nessun cambiamento SHOW/bridge e nessuna consegna finale.

Core35 pubblicato in PR #11, commit `7d1a3a6`, gate locale 760 test numerici più harness UI. Core36 in corso: conservazione breakpoint end+1 e protezione endpoint i64::MAX, entrambi riprodotti; tre nuove regressioni model/comando/Undo superate. Gate completo avviato con ricevitore SHOW `82355f8`. Dettagli `QA/CORE36_AUTOMATION_BOUNDARIES.md`.

Core36: gate completo locale passato (763 test numerici più harness UI, fmt/Clippy/assets/layers/wasm), ricevitore SHOW `82355f8`. Salvataggi, streaming attivato nei loader e tutte le altre fasi restano da concludere; nessuna app finale prodotta.

Core36 pubblicato in PR #12, commit `0721f14`, gate locale 763 test numerici più harness UI. Core37 in corso: backup numerati esclusivi, errore prima di sostituire il progetto quando la copia fallisce, nove regressioni mirate passate. Gate completo avviato con ricevitore SHOW `82355f8`; restore/recovery con media rimane lavoro distinto.

Core37: gate completo locale passato (772 test numerici più harness UI, fmt/Clippy/assets/layers/wasm), ricevitore SHOW 82355f8. Nessuna sostituzione del progetto su fallimento backup; restore/recovery e storage restano aperti. Nessuna consegna finale.

Core37 pubblicato in PR #13, commit 8fe62a3. CI Core34 39942163391, Core35 39944262965 e Core36 39945823565 verificate success sui rispettivi commit. Core37 40024235827 ancora in corso all’ultima verifica. Core38: perdita headroom float riprodotta, codec WAV/AIFF corretto e prove codec/sessione passate; regressione render bridge e gate completo in verifica. Scrittura a blocchi e attivazione loader restano da completare.

Core38: gate completo locale passato (776 test numerici più harness UI, fmt/Clippy/assets/layers/wasm). Regressione stem con headroom passata, mantenendo la legge pan esistente. Nessuna modifica bridge/SHOW.

Core38 pubblicato in PR #14, commit 0dceb0c, gate locale 776 test numerici più harness UI. Core39 in corso: writer WAV/RF64 bounded, SourceAudio.write_wave e salvataggi nativi senza WAV completo in RAM; prove componenti passate, integrazione save e gate da verificare. Bundle/sampler/bridge adapter/loader restano aperti.

Core39: gate completo locale passato (783 test numerici più harness UI, fmt/Clippy/assets/layers/wasm), con ricevitore SHOW 82355f8. Scrittura sorgenti disco attiva nei salvataggi nativi; caricamento disco nei loader ancora non attivo.

Core39 pubblicato in PR #15, commit c6ebb66, gate locale 783 test numerici più harness UI. Core40 in corso: writer bundle generico a file, sorgenti disco senza WAV intermedio e limiti coerenti col lettore; regressioni bundle/errori in esecuzione. Browser mantiene archivio in memoria, apertura nativa bounded e integrazione UI da completare.

Verifica prossima sampler: PoolSample legge soltanto buffer residente. Preparare fuori callback le sorgenti effettivamente assegnate a strumenti, con budget PCM cumulativo esplicito e riuso per identità Arc attraverso aggiornamenti clip/mixer; integrare nelle PreparedMedia e nel sync. Errori di preparazione devono bloccare il playback, senza silenzio implicito. Non attivato né implementato da questa nota.

Core40: gate completo locale passato (786 test numerici più harness UI, fmt/Clippy/assets/layers/wasm), ricevitore SHOW 82355f8. CI Core37 40024235827, Core38 40024912976 e Core39 40025731269 ancora in corso all’ultima verifica.

Core40 pubblicato in PR #16, commit 57c7324, gate locale 786 test numerici più harness UI. Core41 in corso: preparazione PCM degli strumenti fuori callback con budget 64 MiB delle sorgenti disco, riuso e errori; tre test mix mirati passati, test retry player e gate in verifica. Adattatore bridge e loader restano aperti.

Core41: gate completo locale passato (791 test numerici più harness UI, fmt/Clippy/assets/layers/wasm), ricevitore SHOW 82355f8. Cinque regressioni nuove mix/player e aspettativa sampler missing aggiornata al rifiuto nativo esplicito. Un aggiornamento sessione rifiutato impedisce PLAY sulla snapshot precedente finché non arriva un aggiornamento valido. Nessuna modifica SHOW/bridge; piano completo ancora aperto.

Core41 pubblicato in PR #17, commit 7589b39, gate locale 791 test numerici più harness UI. Core42 in corso: snapshot selettiva e motore query temporaneo per compatibilità del renderer bridge invariato con sorgenti disco. Prime tre regressioni passate; estensione marker/media change e gate in verifica. Budget offline esplicito, non renderer bridge streaming.

Audit consumatori prima dei loader: clip_group_file::export codifica ancora buf.buffer e l’import determina formato da buffer.num_channels, poi limita a mono/stereo. Adattare export a write_wave/salvataggio esclusivo, formato backend-aware e conversione completa dei tempi fade/sync/envelope nel gruppo prima dell’attivazione loader. Percorsi save/bundle wasm restano intenzionalmente residenti; sampler nativo già preparato. Nessuna attivazione anticipata.

Core42: gate completo locale passato (794 test numerici più harness UI, fmt/Clippy/assets/layers/wasm), ricevitore SHOW 82355f8. Renderer/publisher/schema bridge e SHOW invariati; snapshot temporanee con budget, ordine/ID/marker e documento verificati. Loader e fasi successive restano aperti.

Core42 pubblicato in PR #18, commit 5ed6e38, gate locale 794 test numerici più harness UI. Core43 in corso: export gruppi di clip da sorgenti disco, formato multicanale, conversione completa tempi e preservazione overlap. Tre regressioni nuove/cinque test mirati passati; gate completo avviato. Decodifica import gruppi e loader generali restano residenti, da integrare.

Core43: gate completo locale passato (797 test numerici più harness UI, fmt/Clippy/assets/layers/wasm), ricevitore SHOW 82355f8. CI Core37 40024235827 verificata success su 8fe62a3; Core41 40027730182 e Core42 40028485716 ancora in corso. Nessuna modifica SHOW/bridge o consegna finale.
