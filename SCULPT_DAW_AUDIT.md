# Sculpt DAW — audit tecnico

Data: 19 ottobre 2026. Base esaminata: Core27, `f77519ce482007d20c3d8738be3826bce7132727`, ultimo commit del branch di distribuzione `feature/core27-io-setup` verificato sul remoto. Non è la vecchia `main`. Questo documento avvia la Fase 1 del piano richiesto; non certifica la DAW per impiego professionale e non dichiara completate le fasi successive.

## Metodo e limiti

Ispezione dei percorsi effettivamente compilati, confronto con README, ROADMAP, documentazione di parità, QA, workflow e issue aperte del repository SculptDAW. La CI della base Core27 è stata verificata il 19 ottobre: [run 39173098624](https://github.com/mrgianluca990-bit/SculptDAW/actions/runs/39173098624), conclusione `success`, SHA corrispondente. I suoi risultati non sono test della nuova correzione.

Stati: **presente** = percorso implementato identificato; **parziale** = limiti concreti nel codice; **bug confermato** = comportamento errato ricostruibile nel percorso; **da collaudare** = manca prova pratica sufficiente. La presenza di un comando o di un test non dimostra tutte le combinazioni hardware/plugin.

Nessuna modifica a SHOW, al protocollo, al renderer di pubblicazione o al codice del bridge. I test di contratto esistenti rimangono regressioni. Nessun sorgente o asset proprietario esaminato. L'analisi comparativa delle altre DAW e degli ultimi sviluppi SoundCraft è una fase successiva: non viene sostituita da percentuali di parità ereditate.

I file locali `plan/STATUS.md`, `plan/execution-plan.md` e il checkout `../../craftrules` citati dall'AGENTS upstream non sono disponibili. Sono stati utilizzati il codice e i documenti presenti; non sono stati inventati contenuti mancanti.

## Architettura effettiva

- `time`, `audio-io`, `midi`: tempo, decodifica/codifica audio e MIDI file.
- `dsp`, `model`: elaborazione e documento di sessione. `Session.pool` conserva audio decodificato condiviso.
- `mix`: elaborazione comune realtime/offline, bus, aux, insert e compensazione delle latenze dichiarate dai plugin.
- `engine`: comandi, editing, Undo, import/export, salvataggio e riapertura.
- `playback`: CPAL/CoreAudio, callback output, cattura input e writer su disco. Input e output sono stream distinti.
- `ui-egui`: un `SoundApp` con un Engine; Edit e Mix leggono lo stesso documento. Le finestre native dei plugin hanno un proprio ciclo di vita.
- `vst3-host`, `au-host`, `clap-host`: FFI isolate in crate, ma plugin caricati nel processo della DAW. Una crate separata non è isolamento da crash nativi.
- `apps/soundcraft` (package `sculpt-daw`) e CLI incorporata: prodotto distribuito; i nomi SoundCraft dei crate non implicano che sia stata integrata tutta la versione upstream 0.8.

## Stato e interventi

| Componente | Stato ed evidenza nel codice | Criticità | Intervento suggerito |
|---|---|---|---|
| Selezione dispositivi | Presente. `playback/src/sculpt_audio_settings.rs::available_devices/stream_config`, `ui-egui/src/windows.rs::apply_audio_settings`: capacità F32, rate/buffer combinati, trial prima del salvataggio. | P0, collaudo hardware pendente | Verificare interfaccia multicanale e cambi connessione; non reimplementare Core26. |
| Buffer | Presente la richiesta CPAL secondo capacità, fino a 2048; callback output misura frame osservati. Il quantum interno massimo è 512, distinto dal buffer hardware. | P0, misura incompleta | Confrontare richiesto/osservato ai diversi rate; misurare round-trip prima di dichiarare latenza. |
| Sample rate | Parziale. Ingresso segue profilo/rate sessione; output rifiuta rate non supportato. `AudioState` converte quando device e sessione differiscono; `InputRing::pop_into` interpola monitoraggio a rate differenti. | P0, SRC ancora presente | Progettare separatamente politica di rate e migrazione offline DAW, senza promettere assenza di SRC. |
| Clock/duplex | Parziale: due stream CPAL, FIFO monitor e interpolazione; nessuna compensazione esplicita di deriva clock tra dispositivi separati individuata. | P0 | Prova lunga con input/output distinti, drift e misure; successivo progetto duplex coerente. |
| Guasto input durante REC | **Bug confermato sulla base**: il callback errori in `record.rs` scriveva solo un log; `SoundApp::handle_transport` non riceveva il guasto; `Recorder::arm` poteva riusare lo stream fallito. | **P0, primo intervento Core28** | Segnale atomico persistente, stop REC e finalizzazione take, avviso e riapertura esplicita. |
| Guasto output | Parziale: `Player::stream_failed` esiste ed è mostrato nel Playback Engine; il trasporto non ha un percorso completo di recovery automatico del dispositivo. | P0, distinto dalla correzione input | Definire stop/recovery esplicito e testare disconnessione durante playback; nessuna riconnessione silenziosa. |
| Registrazione su disco | Presente: `Recorder::arm`, `CaptureRing`, `write_takes`, `TakeWriter`; ring limitato, WAV float e header aggiornato circa ogni secondo. Non accumula l'intera cattura in RAM. | P0, limiti residui | Verificare stop al confine callback, errori writer/disco pieno e campioni persi in coda. Non considerarli già risolti. |
| Durata take / memoria | Parziale: al termine `engine::io::add_recording` legge e decodifica tutta la take nel pool. WAV/RF64 hanno supporto nativo, ma ciò non elimina il picco RAM al termine di una take lunga. | P0 | Streaming delle sorgenti e gestione del caricamento dopo stop; benchmark di RAM. |
| Recovery take | Parziale: `create_new` evita sovrascrittura e gli header periodici conservano un prefisso leggibile. Non è stato individuato un journal che reinserisca automaticamente nella sessione tutte le take di un processo interrotto. | P0 | Crash controllato con media sintetici, recupero esplicito e riconciliazione dei file. Header aggiornato non equivale a `fsync` per ogni secondo. |
| Punch / loop record | Presenti in `SoundApp::start_recording/finish_recording`, `io::add_recording`; punch rifila dopo cattura, loop suddivide in playlist. Latenza di avvio input/output non calibrata. | P0, da collaudare | Impulso/loopback per offset di registrazione e confini; non dichiarare precisione campione senza misura. |
| Playback da disco | **Mancante**: `open_session/load_source_audio` leggono e decodificano file interi; `mix::render_clip` legge il buffer del pool. Nessun prefetch di blocchi dal disco nel percorso realtime individuato. | P0, limite architetturale | Cache limitata, worker di lettura e prefetch, policy seek/underrun; intervento separato dalla segnalazione guasti. |
| Callback realtime | Parziale. Nessun lock bloccante per meter/monitor nel percorso ordinario, ma `AudioState::drain`, `MixEngine::sync` e cambi struttura possono allocare nel callback; invio garbage/comandi usa `std::mpsc`. | P0 | Misurare allocazioni e deadline; spostare preparazione strutture fuori callback, coda limitata e pensionamento fuori realtime. |
| Parallelismo mixer | `mix::run_strips` usa Rayon per almeno quattro strip: il callback attende il completamento dei worker. Non è scheduling realtime dedicato. | P0, rischio, dropout non misurato | Benchmark con carico concorrente e buffer piccoli; decidere in base a misure se introdurre worker dedicati. |
| Routing hardware / bus | Presente Core27: percorsi con ID, mono/stereo, disponibilità, bus→output, rimappatura Main solo realtime; Apply atomico/Undo. `model/src/io_paths.rs`, `engine/src/cmd/io_setup.rs`, `mix/src/lib.rs`. | P0, collaudo multicanale pendente | Ripetere matrice dopo save/reopen, interfaccia ridotta, input assente; conservare identità e ordine tracce. |
| Routing avanzato | Parziale: aux, send pre/post, master, mute/solo, pan e fasi presenti; sottopercorsi, insert hardware calibrati e registrazione da bus interni non implementati. | P1 | Casi audio indipendenti per le funzioni presenti; espansione solo dopo baseline. |
| PDC | Presente per latenze dichiarate, incluso master da Core25; test `mix/tests/master_insert_latency.rs`. Non è compensazione del round-trip hardware né prova di tutti i grafi/plugin. | P0/P1 | Impulsi diretti/bus/aux/send e plugin con latenza variabile; verificare automazione/bypass. |
| Editing / mixer | Comandi effettivi per selezione multipla, gruppi, fade/crossfade, trim, modi editing e Undo. Core23/24 includono correzioni Sculpt; test Core27 non coprono ogni gesto GUI. | P1 | Riprodurre spostamento/cancellazione gruppi e sincronizzazione Edit/Mix; collaudo fader con diverse dimensioni finestra. Nessun nuovo bug grafico dichiarato senza riproduzione. |
| Plugin VST3/CLAP | Processing, stato e API editor presenti; hosting nel processo. VST3 native editor macOS, CLAP GUI floating con limiti dichiarati. | P1, crash nativi possono abbattere DAW | Fixture, salvataggio/riapertura e GUI reali; eventuale isolamento va studiato, non introdotto in questa PR. |
| Plugin AU | Processing e stato presenti; `au-host/src/lib.rs` dichiara editor Cocoa e sidechain/extra bus non ospitati. | P1, funzione incompleta | Non confondere rilevamento AU con editor nativo completo; prova singolo plugin e implementazione dedicata. |
| Firma macOS | Packaging usa entitlement audio input e disable-library-validation, firma ad hoc. L'header del plist dice ancora “loads no unsigned code”: commento obsoleto. | P1, documentazione | Test del bundle estratto con plugin reali. Firma ad hoc non equivale a notarizzazione. Non attribuire FabFilter a entitlement senza prova. |
| Sessioni / backup | `io::write_session/replace_file` salva tramite temporaneo, sync e rename; backup rotanti, autosave da UI escluso durante REC. `.scdaw` e `.scraft` hanno percorsi di apertura presenti. | P1 | Crash/recovery, media mancanti, Finder e Save As; testare migrazione versioni con fixture. |
| Cartelle progetto | Parziale: `write_session` crea `Audio Files` già al primo save anche senza audio; il requisito “solo quando necessarie” non è rispettato in questo punto. | P2 | Correzione distinta, senza ristrutturare cartelle bridge. |
| Import audio | WAV/AIFF/FLAC/MP3 supportati dal decoder; import a rate diverso esegue resampling offline nel pool e comunica i rate. Al save scrive WAV normalizzati. | P1, prova reale pendente | Fixture MP3 e 88.2→48 kHz, durata/pitch/canali, originali preservati, save/reopen; non presentarlo come disk streaming. |
| Sessioni esterne | Importatori completi RPP/PTX/LOGICX/ALS/AAF/OMF non verificati come operativi; non iniziarli in questo intervento. | P2 | Modello intermedio e report perdite soltanto dopo priorità audio. |
| CI e formattazione | Il workflow esegue `cargo fmt --all` prima del gate: verifica un workspace formattato, non l'assenza di differenze di formato nel commit. Tre file di base hanno differenze preesistenti di solo formato. | P2, manutenzione distinta | Dichiarare la preparazione del gate; non aggiungere queste modifiche estranee alla correzione input. |

## Documentazione, duplicazioni e codice ereditato

`README.md` descrive ampiamente funzioni presenti ma non costituisce collaudo. `ROADMAP.md` e `docs/gaps.md` conservano valutazioni upstream del 10 ottobre: “default devices”, “takes held in RAM”, “no PR CI”, entitlement assente e selettori ignorati non descrivono più integralmente Sculpt Core27. I banner Sculpt avvisano del carattere storico; occorre leggere i dettagli sopra, senza trasferire percentuali/stime upstream alla versione attuale.

I file `QA/SculptDAW_Core*_Patch.py` sono patch storiche, non un secondo motore compilato. Il workflow `core25-upstream08.yml` ricostruisce la vecchia base in un job separato quando cambiano i suoi percorsi; `build-macos.yml` compila il workspace corrente e include il gate completo. Non eliminare indiscriminatamente questi file come “codice morto”. Non è stata dimostrata l'inutilità di moduli di produzione tale da giustificarne la rimozione.

L'unica issue/PR aperta trovata nel repository alla verifica è [#1, integrazione Core25](https://github.com/mrgianluca990-bit/SculptDAW/pull/1): descrive una fase precedente, non un elenco affidabile di bug ancora aperti sulla Core27. Le issue numerate in `docs/gaps.md` appartengono all'upstream SoundCraft e non sono state reinterpretate come segnalazioni Sculpt attuali.

## Prima correzione: guasto dello stream di ingresso

Trigger: CPAL invoca il callback errori dello stream input durante una take. Il backend macOS di CPAL 0.16.0 effettivamente compilato registra `kAudioDevicePropertyDeviceIsAlive` per dispositivi espliciti e chiama l'error callback con `DeviceNotAvailable`; la scelta del dispositivo di sistema ha un percorso differente. Prima: solo log, nessuno stop o blocco del riarmo, possibile indicazione REC prolungata senza acquisizione utile. Dopo: latch atomico, monitoraggio silenziato, notifica UI una volta per stream, stop e normale finalizzazione delle take; nuovo REC rifiutato prima di creare file finché l'utente non riapre l'ingresso dal Playback Engine. Il solo playback di clip continua quando non si stava registrando.

La reazione della UI avviene al frame successivo, non è uno stop sample-accurate dal callback. Non converte una disconnessione non segnalata dal driver in un evento: timeout di callback e hot-plug senza callback errori restano da affrontare. Nessun retry automatico e nessun cambio di dispositivo o frequenza durante la take.

Test di regressione: invocazione del callback effettivamente passato a CPAL con errore `DeviceNotAvailable`, writer reale e rilettura WAV stereo campione per campione; notifica una sola volta, niente monitoraggio arretrato, niente nuovi file al riarmo fallito. Test UI della politica stop REC / continua playback. Non sostituiscono una disconnessione fisica.

## Ordine dei prossimi interventi

1. Concludere gate e distribuzione della correzione input, poi collaudo di disconnessione e recovery esplicita.
2. Regressioni per arresto writer/callback, coda persa e disco pieno; misura offset REC input/output. Correggere un difetto riprodotto per PR.
3. Streaming sorgenti limitato in memoria, includendo caricamento delle take dopo stop, seek e underrun osservabili.
4. Preparazione del grafo fuori callback e benchmark delle deadline/Rayon; politica clock/rate DAW esplicita.
5. Workflow editing/plugin/sessioni con riproduzioni concrete, poi confronto funzionale tra DAW e infine importatori esterni.

Benchmark da eseguire, non risultati acquisiti: stesso progetto sintetico, stesso rate/buffer/dispositivo, numero strip/plugin e durata dichiarati; misurare CPU, RSS, deadline callback e perdite cattura, prima/dopo. Apple Instruments, loopback hardware e prove FabFilter/GUI non sono stati eseguiti in questo audit. Nessun miglioramento percentuale o assenza di dropout viene dedotto dai soli test.

## Registro verifiche Core28

Gate locale sulla correzione finale completato: 713 test con riepilogo numerico superati, oltre ai test UI con harness; fmt, Clippy con `-D warnings`, assets, layers e check WebAssembly superati. Ambiente: Rust 1.99.1, macOS arm64. Il check wasm conserva il warning preesistente sulla variante `CaptureStates` non costruita; non è un nuovo errore della correzione.

Come per Core26/27, il gate è eseguito in un processo sandbox che nega soltanto la lettura delle due cartelle VST3 installate (`/Library/Audio/Plug-Ins/VST3` e `~/Library/Audio/Plug-Ins/VST3`); i test fixture restano attivi. Non significa collaudo dei plugin dell'utente. Il workspace viene formattato prima del gate, coerentemente con il workflow; i tre file di base con modifiche estranee di solo formato sono esclusi dal commit.

Regressioni Rust di I/O e bridge superate; il ricevitore Swift effettivo viene compilato e usato dal workflow macOS. Esito CI sul commit distribuito, verifica dello ZIP e firma/architetture saranno riportati nella checklist della consegna. Disconnessione hardware, ascolto, latenza, Instruments e plugin reali rimangono da collaudare.
