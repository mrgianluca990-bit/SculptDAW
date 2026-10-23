# Core44 — apertura progetti da disco

23 ottobre 2026. Lavoro in corso, non consegna né gate completo superato.

SourcePool possiede una cache lazy tramite owner condiviso con le snapshot Undo anche prima del primo import. Lock/allocazioni/creazione worker solo sul controllo; default cache 32 MiB. L’uguaglianza del documento ignora l’owner runtime. Una nuova pool di progetto ha owner distinto; pool con sorgenti disco già inserite può adottarne la cache.

L’apertura nativa riconosce WAV/RF64/BW64 PCM/float coerenti con la frequenza del progetto e collega SourceAudio disco. Le waveform vengono ancora calcolate sincronicamente a blocchi, massimo 16384 blocchi per sorgente e budget 1048576 ripartito fra i sorgenti dichiarati; non dichiarare apertura istantanea o completamento del lavoro waveform/background. Decoder compressi e rate mismatch mantengono il precedente percorso residente/offline. Import generale, take e conversione/migrazione offline persistente restano successivi.

Save As deve ricollegare le sorgenti attive alle copie verificate nella nuova cartella, senza rifare le waveform. Save Copy non ricollega il progetto aperto. La regressione nuova include cancellazione della vecchia cartella media e Undo/Redo: difetto riprodotto su Undo dopo cancellazione media vecchi, poi corretto ricollegando soltanto riferimenti runtime identici nelle snapshot Undo/Redo. Test passato. Payload diversi con ID riutilizzati non vengono sostituiti; resta da ampliare audit di storia con sorgenti presenti solo in Redo o rimosse dalla pool corrente.

Verificati test cache lazy/concorrenza/Undo vuoto e 35 test I/O esistenti. Gate completo locale passato: 799 test numerici più harness UI, fmt/Clippy/assets/layers/wasm; sandbox con esclusione sola lettura VST3 installati, ricevitore SHOW 82355f8 invariato. Nessuna modifica SHOW/bridge; collaudo UI, misure memoria e hardware ancora distinti e da completare.
