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
