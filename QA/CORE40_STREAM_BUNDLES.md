# Core40 — bundle da sorgenti disco

23 ottobre 2026. In verifica, nessuna consegna finale.

write_session_bundle accetta destinazioni Write+Seek e scrive sorgenti residenti/disco direttamente nel contenitore ZIP Stored, senza WAV intermedio completo. session_bundle resta l’adattatore a byte per il browser e le API precedenti: quel percorso conserva necessariamente l’archivio finale in memoria. Il trasferimento a file permette output a memoria limitata; il chiamante deve pubblicare/sincronizzare il file soltanto dopo successo e rimuovere eventuali parziali.

Preflight sul numero di entry e sul payload audio con moltiplicazioni checked, includendo header WAV e JSON finale: non genera bundle che superano i limiti dell’attuale lettore (10000 entry / 1 GiB espanso). È un limite esplicito dei bundle, distinto dai progetti nativi a cartella. Apertura nativa di bundle grandi con estrazione a blocchi e integrazione UI restano fasi successive; non dichiarare risolto ogni limite di memoria del session management.

Regressioni nuove: sorgente disco sette canali scritta su file bundle e riaperta con PCM identico, documento e pool originale intatti; file sorgente mutato rifiutato; errori di destinazione in header/payload senza modifica del documento; overflow e conteggio esatto del limite. Nove test mirati bundle passati (tre regressioni nuove); gate completo in corso dopo correzione di un lint nel sink di test. Gate completo locale passato: 786 test numerici più harness UI, fmt/Clippy/assets/layers/wasm; lettura VST3 installati esclusa dalla sandbox e ricevitore SHOW 82355f8 invariato. Non è prova hardware/plugin reali.

Nessuna modifica SHOW/bridge o schema sessione. Loader disco, sampler e adattatore bridge ancora da concludere. Collaudo hardware distinto dai test automatici.
