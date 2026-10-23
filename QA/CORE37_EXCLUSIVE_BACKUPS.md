# Core37 — backup esclusivi e protezione del salvataggio

23 ottobre 2026. Nove regressioni mirate e gate completo locale passati: 772 test numerici più harness UI, fmt/Clippy/assets/layers/wasm. Ricevitore bridge SHOW 82355f8 compilato da snapshot, senza modifiche a SHOW. Il gate usa la sandbox che esclude soltanto la lettura dei VST3 installati; non costituisce collaudo hardware/plugin reali.

Riprodotto il candidato backup dell'audit #9: al limite u32 la vecchia saturating_add riutilizzava il nome già presente e la successiva potatura poteva eliminare anche quella copia. La regressione è fallita sulla base precedente con il vecchio backup non più presente.

Il contatore ora è u64 con incremento checked. La destinazione viene riservata con create_new, senza seguire o sostituire file, directory o symlink già esistenti. Collisioni concorrenti avanzano entro un limite esplicito; esaurimento e errori vengono propagati. La copia controlla lunghezza e modifica del sorgente, conserva i normali permessi del file e sincronizza i dati prima di consentire la sostituzione del progetto corrente. Su Unix nasce privata durante la copia. Una copia fallita viene rimossa quando possibile; nessun successo parziale viene restituito.

Il salvataggio non ignora più il fallimento del backup. Il file progetto precedente e le modifiche in memoria restano conservati, inclusi dirty e Undo. La comparazione con il JSON usa un buffer di 8 KiB invece di leggere un'altra copia completa in RAM. La potatura mantiene dieci nomi gestiti dopo una copia riuscita, anche quando la numerazione ha buchi; non elimina altri file della cartella.

Le prove verificano superamento del vecchio limite, esaurimento u64, collisioni con file/directory e symlink, permessi 0600, copie concorrenti, pulizia di output incompleti, confronto dei contenuti e cronologia con numeri mancanti. Il comando session.save viene provato con un errore backup su percorsi .scdaw, .scraft e legacy senza estensione: nessuna sostituzione del file, nessuna perdita delle modifiche o di Undo. La regressione esistente sulle ultime dieci versioni resta nel gate completo.

La prenotazione del nome è atomica; questa fase non introduce un protocollo di pubblicazione del backup interamente atomico contro terminazione di processo durante la copia. Il progetto corrente non viene sostituito prima del completamento della copia. Riconoscimento delle copie interrotte, selezione/ripristino con risoluzione dei media e recovery restano da collaudare/completare nel piano session management. Nessuna modifica SHOW/bridge, nessuna nuova app finale.
