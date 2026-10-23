# Core39 — scrittura WAV a blocchi e salvataggio sorgenti disco

23 ottobre 2026. In verifica, non consegna finale.

FloatWavWriter scrive WAV float32/RF64 con durata u64 nota e scratch massimo 256 KiB. Blocchi uniformi, rate/canali e lunghezza dichiarata vengono controllati prima dell’output; errori di scrittura parziale sono permanenti e finish rifiuta trasferimenti incompleti o flush falliti. Il chiamante gestisce pubblicazione, cleanup e sync: il writer non promette file atomici.

SourceAudio.write_wave trasferisce sorgenti residenti o disco senza copia PCM integrale; massimo 65536 campioni per blocco, lettore disco mantenuto aperto, cancellazione e verifica finale della versione sorgente. I salvataggi nativi usano questo percorso, create_new, sincronizzazione e rimozione output falliti. Confronto con WAV esistenti a blocchi di 8 KiB per riuso senza leggere tutto il file; collisioni avanzano senza sovrascrittura. I file creati sono ancora visibili durante scrittura: recovery/pubblicazione resistente a crash è una fase distinta.

Sette regressioni mirate passate: output identico all’encoder su mono/stereo/7/64 canali, RF64 header con durata enorme senza payload, non-finiti/headroom, blocchi errati e incompletezza, errori write/flush; sorgente disco/residente identica, cancellazione e mutazione finale; salvataggio/copia/riuso su 7ch e errore media con documento intatto e output parziale rimosso. Gate completo locale passato: 783 test numerici più harness UI, fmt/Clippy/assets/layers/wasm. Sandbox esclude soltanto lettura VST3 installati; ricevitore invariato della consegna SHOW 82355f8. Non prova plugin reali/hardware.

Restano bundle, sampler, adattatore offline bridge e caricamento disco nei loader. Nessuna modifica SHOW o codice/protocollo bridge. Nessun collaudo hardware/ascolto dedotto dai test.
