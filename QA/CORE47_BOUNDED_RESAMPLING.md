# Core47 — SRC offline per finestre

24 ottobre 2026. Quattro regressioni nuove e Clippy DSP passati; gate completo locale passato: 815 test numerici più harness UI, fmt/Clippy/assets/layers/wasm; sandbox con esclusione sola lettura VST3 installati, ricevitore SHOW 82355f8 invariato.

WindowedResampler riusa il kernel sinc Kaiser esistente, senza cambiare il resample whole-buffer precedente. Posizioni output globali e filtro con supporto completo nei blocchi: i confini del trasferimento non diventano confini audio. Richiede una finestra sorgente completa; dati mancanti nel file sono errore, solo i veri bordi sorgente implicano zero padding. Rate identiche copiano i campioni senza filtro.

Durata calcolata con aritmetica u128 e arrotondamento razionale; indici u64 con limite 2^53 per conversione esatta delle posizioni intere a f64. Stage per finestra/blocco massimo 1048576 campioni complessivi, intervallo input fino a 65536 frame, dimensione consigliata adattata a rate/canali/supporto filtro. Nessun limite whole-buffer ereditato che tronchi silenziosamente una conversione lunga. Il chiamante sceglie min(blocco consigliato, frame rimanenti). Errori su rate/canali/shape/finestre mancanti/non-finiti e overflow, nessun output parziale restituito.

Prove: confronto bit-per-bit con sinc precedente su 1/2/7/64ch, 44,1→48, 88,2→48, 192→44,1, 48→96 e identità; impulso senza contaminazione canali e posizione attesa; indici oltre 32 bit e partizionamento equivalente; budget raccomandati da 8 a 768 kHz, input vuoto ed errori. Non sono misure hardware né una nuova qualificazione della risposta in frequenza di ogni ratio: il filtro resta quello esistente, incluso cutoff minimo 0,01.

È un componente DSP offline; collegamento a decoder/file writer/import, progress UI e migrazione legacy ancora da fare. Nessuna SRC aggiunta nel callback, nessuna dichiarazione di rimozione generale della SRC DAW. SHOW/bridge invariati, nessuna consegna finale.
