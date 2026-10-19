# SCULPT DAW
## Piano di sviluppo, reverse engineering e ottimizzazione professionale

**Data:** 19 ottobre 2026

**Repository principale:**
https://github.com/mrgianluca990-bit/SculptDAW

**Piattaforma prioritaria:** macOS Apple Silicon

**Tecnologie principali:** Rust, Core Audio, egui, VST3, Audio Units, CLAP.

---

# 1. OBIETTIVO

Trasformare Sculpt DAW in una Digital Audio Workstation professionale, stabile e completa.

Il software deve combinare:

- Workflow di editing e mixaggio ispirato a Pro Tools.
- Routing avanzato paragonabile a REAPER.
- Gestione dei plugin professionale.
- Registrazione multitraccia affidabile.
- Interoperabilità con altre DAW.
- Prestazioni ottimizzate per Apple Silicon.
- Interfaccia coerente con l'identità Sculpt.

La filosofia è quella di mantenere un software potente, ma semplice da utilizzare.

**Non vogliamo una copia di Pro Tools. Vogliamo una DAW che possa sostituirlo nei normali lavori professionali.**

### Vincolo importante

Il collegamento con Sculpt Show attualmente soddisfa i requisiti dell'utente.

Non intervenire sul Bridge, sui suoi protocolli o sul repository Sculpt Show.

Qualsiasi modifica della DAW deve preservare la compatibilità con il Bridge esistente.

---

# 2. FASE 1 — AUDIT TECNICO

**Priorità P0**

Prima di sviluppare nuove funzionalità, analizzare il codice realmente presente nell'ultimo commit.

Esaminare:

- Architettura Rust.
- SoundCraft e modifiche specifiche Sculpt.
- Motore audio.
- Mixer.
- Editing.
- Routing.
- Plugin hosting.
- Gestione sessioni.
- Interfaccia.
- GitHub Actions.

Confrontare il codice con README, ROADMAP, documentazione QA e issue.

Non considerare attendibile una funzionalità soltanto perché dichiarata nella documentazione.

### Attività

1. Identificare le funzionalità operative.
2. Individuare i componenti incompleti.
3. Rilevare codice duplicato o non utilizzato.
4. Identificare possibili regressioni.
5. Verificare la copertura dei test.
6. Individuare i problemi architetturali.
7. Separare bug reali da funzionalità non ancora implementate.

### Risultato

Creare `SCULPT_DAW_AUDIT.md`.

Il documento dovrà contenere una tabella con stato, criticità, componente interessato e intervento suggerito.

---

# 3. FASE 2 — MOTORE AUDIO PROFESSIONALE

**Priorità P0 — Fondamentale**

Questa è la fase più importante del progetto.

L'obiettivo è ottenere un motore audio affidabile, prevedibile e adatto alla registrazione professionale.

## 3.1 Core Audio

Analizzare e ottimizzare:

- Riconoscimento dei dispositivi audio.
- Selezione input/output.
- Buffer size.
- Sample rate.
- Gestione dei clock.
- Latenza.
- Compensazione della latenza.
- Audio multicanale.
- Gestione dei dispositivi scollegati.
- Recovery dagli errori.

Verificare che le impostazioni del Playback Engine siano effettivamente applicate al dispositivo.

### Buffer da supportare

32 / 64 / 128 / 256 / 512 / 1024 / 2048 campioni, secondo le capacità del dispositivo.

### Sample rate

44.1 / 48 / 88.2 / 96 / 176.4 / 192 kHz, quando supportati dall'hardware.

## 3.2 Registrazione

Verificare e, dove necessario, implementare:

- Registrazione direttamente su disco.
- Registrazione multitraccia.
- Monitoring.
- Punch In/Out.
- Loop recording.
- Compensazione della latenza di registrazione.
- Gestione delle take.
- Recovery delle registrazioni interrotte.
- Protezione contro la perdita dei file.

Evitare di mantenere intere registrazioni in RAM.

## 3.3 Playback

Analizzare:

- Streaming audio da disco.
- Prefetch.
- Buffer underrun.
- Dropout.
- Thread realtime.
- Scheduling.
- Gestione delle operazioni durante playback.
- CPU load.

Utilizzare Apple Instruments e benchmark automatizzati.

## 3.4 Routing

Verificare:

- Ingressi mono e stereo.
- Tracce multicanale.
- Bus.
- Aux.
- Master.
- Send pre/post fader.
- Routing hardware.
- Sidechain.
- Pan law.
- Phase invert.
- Mute e solo.
- Plugin Delay Compensation.

Il routing deve mantenersi corretto dopo salvataggio e riapertura della sessione.

### Criterio di completamento

Il motore deve superare test automatici e prove audio reali senza errori di routing, perdita di campioni o interruzioni non previste.

---

# 4. FASE 3 — REVERSE ENGINEERING DELLE DAW

**Priorità P1**

Questa fase serve a studiare le soluzioni adottate dalle applicazioni professionali e migliorarne l'equivalente in Sculpt.

Non copiare codice o risorse proprietarie.

Utilizzare analisi comportamentale, API documentate, progetti di prova e sorgenti open source compatibili.

## 4.1 Pro Tools

Studiare:

- Edit Window.
- Mix Window.
- Gestione delle tracce.
- Gruppi.
- Playlist.
- Clip editing.
- Automation.
- I/O Setup.
- Plugin inserts.
- Sends.
- Import Session Data.
- Gestione dei progetti.

Concentrarsi sul comportamento delle funzioni e sull'ergonomia.

Costruire test equivalenti all'interno di Sculpt.

## 4.2 REAPER

Studiare:

- Routing matrix.
- Gestione flessibile dei canali.
- Editing multitraccia.
- Grouping.
- Item editing.
- Actions.
- Undo.
- Gestione plugin.
- Performance del motore audio.

REAPER rappresenta uno dei principali riferimenti per flessibilità e personalizzazione.

## 4.3 Logic Pro

Studiare:

- Gestione MIDI.
- Software instruments.
- Editing.
- Flex Time.
- Automazioni.
- Plugin Audio Unit.
- Gestione della sessione.

## 4.4 Ardour

Analizzare direttamente il sorgente disponibile.

Concentrarsi su:

- Audio engine.
- Disk streaming.
- Routing.
- Session management.
- Plugin processing.
- Recording.

Studiare le soluzioni architetturali senza introdurre dipendenze incompatibili con le licenze di Sculpt.

## 4.5 SoundCraft

Analizzare gli sviluppi più recenti.

Confrontare le implementazioni originali con le modifiche già presenti in Sculpt.

Integrare soltanto miglioramenti realmente utili, evitando di sovrascrivere funzionalità specifiche di Sculpt.

### Output

Creare una matrice comparativa:

| Funzione | Pro Tools | REAPER | Logic | Sculpt |
|---|---|---|---|---|
| Editing | Valutazione | Valutazione | Valutazione | Stato reale |
| Routing | Valutazione | Valutazione | Valutazione | Stato reale |
| Plugin | Valutazione | Valutazione | Valutazione | Stato reale |
| MIDI | Valutazione | Valutazione | Valutazione | Stato reale |
| Recording | Valutazione | Valutazione | Valutazione | Stato reale |

La matrice deve riportare anche le evidenze tecniche utilizzate.

---

# 5. FASE 4 — EDITING E MIXER

**Priorità P1**

Migliorare l'efficienza operativa dell'interfaccia mantenendo il workflow Sculpt già sviluppato.

## Edit Window

Verificare e perfezionare:

- Selezione multipla.
- Spostamento simultaneo di clip.
- Editing di gruppo.
- Crossfade.
- Fade In/Out.
- Trim.
- Slip.
- Shuffle.
- Spot.
- Grid.
- Zoom.
- Marker.
- Timeline.
- Drag & drop.
- Undo/Redo.

### Problemi specifici da verificare

**Fader della Edit Window**

Controllare le sovrapposizioni grafiche e il corretto ridimensionamento.

**Selezione multipla**

Verificare che le operazioni su più clip selezionate funzionino come previsto.

**Group editing**

Verificare selezione, spostamento, cancellazione e modifica simultanea.

**Mix/Edit synchronization**

Le modifiche effettuate in una vista devono riflettersi correttamente nell'altra.

## Mix Window

Verificare:

- Riordinamento delle tracce.
- Fader.
- Meter.
- Pan.
- Insert.
- Sends.
- Routing.
- Groups.
- Aux.
- VCA.
- Master.
- Drag & drop dei plugin.

### Gestione delle finestre

Mantenere un solo progetto attivo per istanza dell'applicazione.

Edit e Mix devono utilizzare lo stesso stato di sessione, senza creare progetti indipendenti.

Ridurre le finestre superflue, preservando però quelle necessarie agli editor dei plugin.

---

# 6. FASE 5 — PLUGIN HOSTING

**Priorità P1**

Il sistema deve supportare in maniera affidabile i plugin:

- VST3.
- Audio Units.
- CLAP.

### Test fondamentali

1. Scansione.
2. Caricamento.
3. Attivazione.
4. Elaborazione audio.
5. Interfaccia grafica.
6. Meter.
7. Automazione.
8. Bypass.
9. Gestione preset.
10. Salvataggio dello stato.
11. Chiusura e riapertura della sessione.
12. Rimozione.
13. Gestione degli errori.

### Problemi da verificare

- Plugin che vengono rilevati ma non caricati.
- Editor grafici incompleti.
- Meter e animazioni non funzionanti.
- Problemi con plugin FabFilter.
- Drag & drop degli insert.
- Comportamento dei plugin durante playback.
- Eventuali crash.

Verificare entitlement e compatibilità delle build firmate macOS.

Utilizzare Pluginval dove applicabile.

### Architettura

Studiare la possibilità di isolare i plugin più problematici, evitando che un crash del plugin provochi la perdita della sessione.

Non introdurre un sistema di sandboxing senza prima valutarne latenza, CPU e compatibilità.

---

# 7. FASE 6 — COMPATIBILITÀ DELLE SESSIONI

**Priorità P2**

Uno degli obiettivi strategici di Sculpt DAW è aprire sessioni provenienti da altri software.

Non tentare immediatamente la compatibilità completa.

Costruire importatori progressivi, verificabili e modulari.

### Ordine di sviluppo

**1. REAPER — RPP**

Partire dal formato testuale di REAPER.

Analizzare le strutture delle sessioni e costruire un importatore.

**2. Pro Tools — PTX/PTF**

Valutare `ptformat` e altri strumenti disponibili.

Studiare l'importazione di tracce, regioni, media e timeline.

**3. Logic Pro — LOGICX**

Analizzare i parser disponibili e la struttura dei progetti.

**4. Ableton Live — ALS**

Studiare i progetti di conversione esistenti.

**5. AAF/OMF**

Implementare interoperabilità standard per audio e postproduzione, dopo aver verificato le librerie disponibili.

### Architettura richiesta

Creare un modello intermedio:

`External Session → Format Parser → Sculpt Session Model → Native Session`

Ogni importatore deve conservare il massimo numero possibile di informazioni.

### Informazioni da preservare

- Tracce.
- Nomi.
- Colori.
- File audio.
- Posizioni delle regioni.
- Fade.
- Marker.
- Tempo.
- Metrica.
- Routing.
- Automazioni.
- Informazioni sui plugin, quando recuperabili.

Se un'informazione non può essere recuperata, segnalarlo nel report di importazione.

Non inventare dati e non associare automaticamente plugin differenti senza autorizzazione.

---

# 8. FASE 7 — SESSION MANAGEMENT

**Priorità P1**

Verificare il funzionamento del formato nativo Sculpt.

Ogni nuovo progetto deve avere una cartella dedicata.

Struttura indicativa:

SCULPT PROJECT/
- Project.scdaw
- Audio Files/
- Rendered Files/
- Session Backups/

Le directory devono essere create soltanto quando necessarie.

### Funzioni richieste

- New Session.
- Open Session.
- Save.
- Save As.
- Import Session Data.
- Backup.
- Autosave.
- Recovery.
- Consolidate.
- Gestione file mancanti.

Verificare anche l'apertura delle sessioni direttamente dal Finder.

Preservare la compatibilità con i precedenti progetti `.scraft` e con il formato già adottato da Sculpt.

### Importazione audio

Controllare:

- WAV.
- AIFF.
- FLAC.
- MP3.
- File multicanale.
- Sample rate differenti.
- Conversione automatica quando necessaria.
- Segnalazione di formati non supportati.

In particolare, verificare i problemi precedentemente segnalati nell'importazione di MP3 e WAV a 88.2 kHz in sessioni a 48 kHz.

---

# 9. FASE 8 — AUTOMATED QUALITY ASSURANCE

**Priorità P0, trasversale a tutte le fasi**

Creare una suite di test automatizzati.

### Audio

- Null test.
- Impulse response.
- Stereo integrity.
- Multichannel routing.
- Pan law.
- Gain accuracy.
- Plugin latency.
- Automation timing.
- Bounce accuracy.
- Playback stability.

### Sessioni

- Salvataggio.
- Riapertura.
- File mancanti.
- File danneggiati.
- Compatibilità con vecchie versioni.
- Ripristino dopo interruzione.

### Editing

- Undo/Redo.
- Group editing.
- Spostamento clip.
- Fade.
- Crossfade.
- Marker.
- Drag & drop.

### Plugin

- Loading.
- Processing.
- GUI.
- State recall.
- Crash handling.

### Benchmark

Misurare CPU, RAM e comportamento realtime su progetti di riferimento.

Registrare le condizioni del test e confrontare le misurazioni prima e dopo le modifiche.

Non dichiarare miglioramenti basandosi su impressioni.

---

# 10. PRIORITÀ OPERATIVE

| Ordine | Attività | Priorità |
|---|---|---|
| 1 | Audit dell'ultima versione | P0 |
| 2 | Suite di regressione | P0 |
| 3 | Core Audio e dispositivo audio | P0 |
| 4 | Registrazione e disk streaming | P0 |
| 5 | Stabilità del playback | P0 |
| 6 | Routing e mixer | P0 |
| 7 | Bug dell'editing | P1 |
| 8 | Plugin hosting | P1 |
| 9 | Interfaccia e finestre | P1 |
| 10 | Session management | P1 |
| 11 | Reverse engineering funzionale | P1 |
| 12 | Importazione sessioni esterne | P2 |
| 13 | Ottimizzazioni avanzate | P2 |

Le attività indipendenti possono essere sviluppate separatamente, mantenendo controllate le modifiche condivise.

---

# 11. REGOLE PER CODEX

1. Lavorare esclusivamente sul repository SculptDAW.

2. Non modificare Sculpt Show o il Bridge.

3. Non riscrivere componenti già funzionanti senza dimostrarne la necessità.

4. Evitare regressioni.

5. Utilizzare codice esistente quando è tecnicamente appropriato e legalmente compatibile.

6. Evitare modifiche cosmetiche non richieste.

7. Compilare e testare ogni modifica prima di considerarla completata.

8. Non avviare build macOS costose dopo ogni piccola modifica: eseguire prima i controlli locali applicabili.

9. Utilizzare GitHub Actions per le verifiche e le distribuzioni.

10. Limitare il consumo di crediti e risorse, evitando analisi ripetitive.

11. Non introdurre dipendenze senza una motivazione concreta.

12. Documentare ogni intervento rilevante.

13. Mantenere il codice modulare e verificabile.

14. Non dichiarare funzionante una caratteristica senza test adeguati.

15. Privilegiare i problemi che compromettono il lavoro audio reale rispetto alle funzionalità accessorie.

---

# 12. MODALITÀ DI ESECUZIONE

Procedere autonomamente attraverso piccoli interventi consecutivi.

Per ciascun intervento:

**Analisi → Implementazione → Test → Verifica regressioni → Commit → Pull Request**

Non mescolare correzioni indipendenti nella stessa pull request.

Ogni PR deve descrivere:

- Problema affrontato.
- Modifica effettuata.
- Componenti interessati.
- Test eseguiti.
- Risultati.
- Eventuali problemi ancora presenti.

La compilazione macOS deve essere accompagnata, quando necessario, da una checklist di verifica manuale.

Non considerare un test automatico equivalente a una prova hardware reale.

---

# 13. PRIMO INTERVENTO DA ESEGUIRE

Iniziare dalla FASE 1.

Leggere l'ultimo commit del repository e analizzare la reale architettura audio della DAW.

Confrontarla con la documentazione e identificare i problemi critici ancora presenti.

Prestare particolare attenzione a:

1. Gestione Core Audio.
2. Selezione dei dispositivi.
3. Dimensioni del buffer.
4. Routing hardware.
5. Registrazione su disco.
6. Streaming delle sorgenti audio.
7. Stabilità durante playback.
8. Plugin hosting macOS.

Individuare il primo problema confermato che impedisce un utilizzo professionale.

Realizzare una correzione circoscritta, accompagnata da test di regressione.

Aprire una pull request.

**Non iniziare dalla compatibilità con altre DAW finché il motore audio e i principali workflow non risultano sufficientemente affidabili.**

## RISULTATO FINALE ATTESO

Una Sculpt DAW che permetta di:

- Registrare multitraccia.
- Editare in modo efficiente.
- Gestire routing complessi.
- Utilizzare plugin professionali.
- Mixare e automatizzare.
- Importare progetti da altre DAW.
- Salvare e riaprire sessioni senza perdere informazioni.
- Lavorare stabilmente su macOS.

Il raggiungimento di questi obiettivi deve essere dimostrato con test e verifiche, non soltanto con la presenza delle funzioni nell'interfaccia.