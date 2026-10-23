# Core34 — regioni offline e time-stretch su disco

23 ottobre 2026. Revisione verificata. Gate completo locale superato: 756 test numerici più harness UI, fmt/Clippy/assets/layers/wasm. Rust 1.99.1 su macOS arm64; sandbox dei test che esclude soltanto i VST3 installati, fixture attive. Regressioni bridge con validatore Swift compilato dalla copia SHOW `9eb1a57`, senza modificarla. Workspace formattato prima del gate e tre differenze di formato preesistenti escluse dal commit; warning wasm preesistente su CaptureStates. Nessuna nuova build universal o prova hardware.

`SourceAudio::read_region` prepara soltanto l'intervallo richiesto entro un budget totale di campioni esplicito. Mantiene la frequenza originale, controlla canali uniformi e dimensioni, legge a blocchi di al massimo 65.536 campioni complessivi e non materializza il resto della sorgente. Restituisce padding soltanto oltre la fine già nota: una lettura incompleta in-file restituisce errore. La variante con progress consente cancellazione prima dell'allocazione, tra i blocchi e prima del risultato. Il media su disco è verificato anche dopo la lettura, così un file modificato al termine non viene accettato come snapshot coerente.

La copia residente di compatibilità riusa questo percorso e lascia immutata la sorgente originale e le sue waveform limitate. Non è un'API realtime: il risultato della regione è residente, richiede un worker o il thread di controllo e un budget adeguato.

Il time-stretch legge il range effettivo anziché accedere a `buffer.channels`, segnala media mancanti o modificati e richiede un rate coerente con il progetto. Verifica il limite per canale del DSP e il budget totale prima della preparazione: il precedente resize a lunghezza richiesta avrebbe potuto aggiungere silenzio dopo un risultato troncato dal DSP. Ora un risultato incompleto è un errore. Nessuna modifica dell'algoritmo WSOLA o SRC di playback in questa fase.

Session Audio Health usa i metadati del backend e non considera vuota una sorgente su disco solo perché non possiede PCM residente. Mantiene il contratto di query senza filesystem, modifiche al documento, dirty o Undo; non certifica la presenza attuale del file.

## Prove mirate

Due nuove regressioni model: regioni disco/residente identiche, inclusi offset, padding EOF e richieste superiori al singolo blocco; budget/overflow rifiutati, cancellazione e modifica del file alla conclusione senza risultati parziali.

Due nuove regressioni engine: time-stretch identico su mono/stereo/7ch senza rendere residente la sorgente originale; salute corretta e query invariata su questi backend. Media mancanti/modificati e output oltre il limite del DSP rifiutati prima di aggiungere una nuova sorgente. Il test verifica il messaggio di budget per distinguere il rifiuto da altri errori.

Salvataggi/copie/bundle/clip-group, sampler e compatibilità per il renderer bridge congelato restano da completare prima di abilitare import/open/take su disco. Nessuna app finale prodotta, nessuna modifica SHOW o bridge. Il piano completo rimane aperto.
