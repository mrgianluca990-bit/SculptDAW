# Core46 — WAV a durata determinata a EOF

24 ottobre 2026. Sei regressioni nuove, suite audio-io e Clippy mirati passati; gate completo locale passato: 811 test numerici più harness UI, fmt/Clippy/assets/layers/wasm; sandbox con esclusione sola lettura VST3 installati, ricevitore SHOW 82355f8 invariato.

GrowingFloatWavWriter richiede una destinazione seekable vuota. Riusa lo staging 256 KiB del writer esistente, controlla dimensioni/overflow, registra la durata ricevuta e riscrive l’header solo dopo flush e verifica di posizione/lunghezza. La lunghezza dell’header resta stabile nella transizione RIFF→RF64. Tutti gli errori di blocco sono permanenti; finish propaga errori write/seek/flush e non restituisce una destinazione riuscita dopo un errore ignorato.

decode_file_to_wave compone il decoder File con questo writer alla frequenza originale, senza SRC. Finalizza soltanto dopo EOF verificato e callback progress/cancellazione finale; errori decoder o annullamento restituiscono errore e lasciano l’header a durata zero. Il chiamante deve eliminare il parziale e sincronizzare/pubblicare dopo successo: non è introdotta pubblicazione filesystem atomica né integrazione nell’import UI.

Prove: writer a blocchi confrontato byte-per-byte con encoder a durata nota su 1/2/7/64 canali; destinazione preesistente preservata, errore blocco latched, errori write/seek/flush; MP3 stereo gapless e mono senza Xing convertiti in WAV con campioni identici al decoder precedente, progress, cancellazione prima e dopo avvio; sorgente troncata in corso con output non finalizzato. Test Unix RF64 con payload sparso e contatore simulato >4 GiB, poi riapertura e lettura ultimo frame: verifica finalizzazione metadati, non benchmark di un trasferimento completo da 4 GiB.

La durata di uno stream sconosciuto non è un certificato sulla sorgente: il chiamante deve gestire ogni errore e EOF. Allocazioni codec terze parti, SRC a blocchi, import/take/legacy conversion e waveform background restano da completare. Nessuna modifica SHOW/bridge, nessuna nuova consegna app o dichiarazione hardware.
