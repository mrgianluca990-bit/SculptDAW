# Core45 — decodifica file a blocchi

24 ottobre 2026. Sei test mirati e Clippy audio-io passati; gate completo locale passato: 805 test numerici più harness UI, fmt/Clippy/assets/layers/wasm; sandbox con esclusione sola lettura VST3 installati, ricevitore SHOW 82355f8 invariato. Componente non ancora collegato all’import generale.

FileDecoder legge sequenzialmente da File, senza copia dell’intero file né PCM integrale. WAV PCM/float/RF64/BW64 usa WavReader nativo; gli altri codec usano Symphonia su MediaSourceStream da 64 KiB. Host staging limitato a 1048576 campioni per blocco, pacchetto accettato fino a 16 MiB, tag/immagini con limiti richiesti di 1 MiB. Le allocazioni interne del codec/demuxer restano terze parti: non dichiarare contenimento assoluto di memoria o isolamento di processo.

Errori codec/demuxer, cambio formato, durata dichiarata incompleta e modifiche osservabili di lunghezza/mtime sono errori permanenti, senza saltare pacchetti o restituire un EOF riuscito dopo errore. Fine file aggiorna la durata al conteggio effettivo. Il chiamante deve scartare output parziali su errore; non è ancora un importatore né un writer con durata sconosciuta.

Discrepanza riprodotta nel hint AIFF della dipendenza: dichiarava 12005 frame per 12003 decodificati. Il conteggio viene letto dal COMM nativo con scansione header bounded e seek dei corpi; non si disabilita il controllo d’incompletezza. Nessuna copia di sorgenti di terzi o modifica alla dipendenza.

Prove: confronto bit-per-bit col decoder precedente su WAV/AIFF float con headroom e FLAC int24, mono/stereo/7ch, 88200 Hz; RF64/BW64 7ch a 96 kHz; MP3 stereo gapless 5292 frame a 44,1 kHz e mono senza Xing a 48 kHz; dati troncati/modificati, latch dopo errore, 48 varianti MP3 mutate/corte. Fixture MP3 originali sintetiche, generate con script e FFmpeg 7.1 usato solo da sviluppatore in /tmp; binario encoder non incluso nel repository o app. Attribuzione presente; test non richiedono encoder installato.

Rust privato: settings.toml in /tmp non più presente, usato RUSTUP_TOOLCHAIN=stable esplicito senza modificare configurazione utente. Nessuna nuova build universal, nessuna modifica SHOW/bridge. Restano writer durata sconosciuta, SRC offline a blocchi, collegamento import/take/legacy rate e waveform background, oltre alle altre fasi del piano completo.
