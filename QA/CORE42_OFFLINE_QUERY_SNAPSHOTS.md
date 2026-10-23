# Core42 — snapshot offline per query legacy

23 ottobre 2026. In verifica, loader disco ancora non attivo.

La DAW adatta le sorgenti disco al requisito PCM residente del renderer esistente, al confine Engine::execute. Non vengono modificati renderer, publisher, schema/protocollo bridge o SHOW. Solo bridge.render_stems con sorgenti disco richieste usa una Engine temporanea; i normali comandi e le sorgenti residenti seguono il percorso precedente. Preparazione bloccante fuori callback, non un nuovo motore bridge streaming.

SourcePool.resident_snapshot_for prepara soltanto gli ID richiesti, deduplica gli ID, controlla prima il budget cumulativo e preserva gli altri backend. La query usa una copia del documento, gli stessi ID e percorsi, mentre documento originale, cronologia, revision, dirty e journal restano intatti. Anche i panic nella preparazione/query sono convertiti in errori senza sostituire il documento aperto.

Budget esplicito di nuove copie PCM: MAX_RENDER_SAMPLES (attualmente 2 GiB f32), distinto dal budget cache timeline e dal limite degli output offline esistente. La query resta un’operazione offline con copie residenti temporanee; non promettere memoria costante nell’export bridge o supporto oltre questi limiti. File sorgente modificato o budget superato falliscono prima del renderer. Il codice originale mantiene i suoi limiti e la propria gestione di file parziali in caso di errore durante il render.

Prove mirate: snapshot selettiva con ID duplicati, sorgenti non richieste condivise e budget/missing; render mono/stereo da disco confrontato byte-per-byte col renderer da residente, ordine/ID/canali, marker, documento e cronologia; directory non vuota e file sorgente danneggiato; query sintetica che modifica la copia e genera panic senza modificare l’originale. Gate completo locale passato: 794 test numerici più harness UI, fmt/Clippy/assets/layers/wasm, sandbox con sola esclusione lettura VST3 installati e ricevitore SHOW 82355f8 invariato. Round-trip UI/audio reale resta distinto.
