# Core29 — lettore a blocchi, fondazione dello streaming

19 ottobre 2026. `crates/audio-io/src/stream.rs` introduce un lettore nativo WAV/BWF/RF64/BW64 PCM/IEEE float con seek, condivisione della decodifica dei campioni con il codec esistente e letture limitate a 65.536 frame e 64 canali per chiamata. I chunk estranei vengono saltati con seek; i metadati conservati hanno un budget esplicito e un limite di chunk. Byte interi e float mantengono il rate originale: questo componente non esegue SRC.

Le letture completano eventuali short-read del filesystem. Un file troncato dopo l'apertura restituisce un errore, senza inventare silenzio. La normale apertura considera vuoto un data chunk di dimensione zero; il recupero dei PCM di una cattura nota non finalizzata richiede l'API esplicita `open_unfinalized`/`from_unfinalized_reader`. Non si scambia automaticamente per audio una coda di metadati dopo un data chunk vuoto.

Questa è API bloccante per worker/offline, esclusa da wasm. Nessuna chiamata disco deve essere introdotta nel callback. Il lettore non è ancora collegato al player: cache, prefetch, sorgenti, take e waveform restano le attività successive del piano completo. Non dichiarare già risolto il caricamento integrale della DAW.

## Verifiche

- 7 test mirati passati: confronti esatti tra range e decodifica completa per mono/stereo/7 canali, PCM 8/16/24/32 bit e float 32/64; preservazione del rate 88.2 kHz, BWF e normalizzazione dei non-finiti secondo il codec già presente.
- File RF64 sparse oltre 4 GiB: seek e lettura dell'ultimo frame stereo corretti, senza caricare l'audio intero; buffer richiesto per la lettura di prova pari a 8 byte.
- Letture sottostanti di soli tre byte completate correttamente; apertura che salta un JUNK di 2 MiB leggendo meno di 1 KiB di metadati.
- Intestazioni danneggiate, frame parziali, numero eccessivo di chunk, dimensione di lettura fuori limite, RF64 senza dimensione a 64 bit e troncamento dopo apertura verificati.
- Gate locale finale: 720 test con riepilogo numerico superati, oltre ai test UI con harness; fmt/Clippy/assets/layers/wasm passati. VST3 installati esclusi dalla lettura nel processo di test, fixture attive. Validatore Swift effettivo già compilato dalla copia SHOW `9eb1a57` usato nelle regressioni bridge, senza modificare SHOW.

Non è un benchmark del playback né una prova hardware; non è stata eseguita una nuova build universal per questo componente ancora in integrazione. La consegna dell'app è prevista al completamento del piano come richiesto dall'utente.
