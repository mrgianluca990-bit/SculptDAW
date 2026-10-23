# Core43 — gruppi di clip, sorgenti disco e tempi

23 ottobre 2026. Tre regressioni nuove e cinque test mirati passati; gate completo locale passato: 797 test numerici più harness UI, fmt/Clippy/assets/layers/wasm; sandbox con esclusione sola lettura VST3 installati e ricevitore SHOW 82355f8 invariato.

Riprodotta importazione di un gruppo 7ch come stereo sulla base Core42. L’export nativo usa ora write_wave tramite store_source_wav, senza WAV intermedio completo; riusa media identici e risolve collisioni senza sovrascrivere file diversi. Un errore media impedisce la sostituzione del JSON del gruppo. Non è introdotta pubblicazione atomica dell’intero gruppo contro crash o errori della scrittura JSON finale.

Import: formato traccia ricavato dai metadati backend-aware, mantenendo i canali. Conversione razionale checked in i128 per start/length/offset/fade/sync point/gain envelope, con arrotondamento e rifiuto overflow prima della mutazione. Breakpoint che collassano nello stesso campione tengono l’ultimo valore. MIDI conserva i tick musicali. Le clip di ogni lane vengono preparate insieme: cancellazione del vecchio materiale nelle loro aree prima dell’append, quindi sort; le sovrapposizioni interne al gruppo non si troncano più l’una con l’altra.

Validazione di versione, frequenza non zero, timing, riferimenti sorgenti e rate effettivo del WAV; limite JSON 64 MiB. La decodifica media dell’import è ancora residente e la SRC avviene offline tramite add_source: questa fase non abilita né promette un import gruppi interamente bounded. Apertura/import generale/take da disco restano da integrare.

Prove: 96→48 kHz con sette canali, due clip sovrapposte, fade/sync/envelope/offset e Undo/Redo; export da disco con headroom 1,5, collisione e riuso, sorgente corrotta con JSON/documento invariati; versione/rate/timing/overflow invalidi senza modifiche. Nessuna modifica SHOW/bridge o schema .scgrp. Collaudo hardware/ascolto distinto.
