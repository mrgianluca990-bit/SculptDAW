# Sculpt DAW Core #17 — un unico aggiornamento

Questa release usa il commit della Core #15 funzionante come base, riapplica le patch della #16 (incluso Spot) e aggiunge le correzioni della #17. È previsto **un solo job Mac**.

## Errore riscontrato nella #16

L'eseguibile era stato compilato correttamente. L'errore GitHub Actions avveniva dopo il build: il controllo cercava `dist/sculpt/Sculpt DAW.app/Contents/Info.plist`, ma il builder conserva l'app in una cartella temporanea e distribuisce **solo lo ZIP**. La #17 legge il plist direttamente dallo ZIP.

## Installazione senza ricompilazioni intermedie

### Metodo consigliato (un solo commit con GitHub Desktop)

1. Apri il tuo clone locale di `mrgianluca990-bit/SculptDAW` con GitHub Desktop (se non l'hai ancora clonato, usa **File → Clone repository**).
2. Estrai il pacchetto e copia **le due cartelle** `.github` e `assets` nella root del clone, unendo le cartelle e sovrascrivendo **solo** i due file seguenti:
   - `.github/workflows/build-macos.yml`
   - `assets/sculpt-daw/sculpt-daw-512.png`
3. **Non copiare gli altri file del pacchetto nel repository:** sono materiale QA e sorgente della patch per ispezione.
4. In GitHub Desktop controlla che i file modificati siano esattamente questi due, fai un singolo commit e **Push origin**.
5. Apri **Actions** su GitHub. L'unico workflow `Sculpt DAW Core 17 - Consolidated Session Group Markers` compilerà l'app per Mac. Se fallisce, condividi il log (non ricompilare a tentativi).

### Alternativa usando solo GitHub nel browser

È necessario sostituire **entrambi** i file prima di eseguire la build. I due salvataggi sul branch `main` possono avviare due Actions perché il trigger è `push`. Per evitarlo, usa un branch temporaneo, carica l'icona e aggiorna il workflow lì, quindi effettua un **solo merge in main**.

L'icona scelta è la **seconda proposta**, con S metallica e luce ambra. Il file `assets/sculpt-daw/sculpt-daw-512.png` deve essere copiato esattamente al percorso indicato; il workflow ne controlla l'hash SHA-256 prima del lungo build.

**Marcatura ultima riga del workflow:** `# SCULPT_CORE17_WORKFLOW_COMPLETE`. Controlla che compaia su GitHub prima della compilazione, per evitare il troncamento verificatosi con la prima #16.

## Cosa cambia

- **Nuovi progetti** (dialog Save As/Save Copy): `Nome/Nome.scdaw` e sottocartella `Audio Files/`. Le sessioni preesistenti mantengono il loro percorso, inclusi i `.scraft` storici.
- **Importazione tracce**: browser macOS immediato, selezione multipla, azione di importazione unica (senza un secondo pannello intermedio).
- **Marker**: esposti `View > Rulers > Markers` e `Window > Memory Locations`; motore originale di add, edit, recall, next/previous, delete, salvataggio; test di persistenza.
- **Send**: un solo controllo di livello editabile; rimosso il grafico statico duplicato (già previsto dalla patch Core16).
- **Clip/tracce multiple**: trascinando una clip su una traccia inclusa nella selezione, vengono spostate anche le clip sovrapposte sulle altre tracce selezionate; selezioni esplicite di clip mantenute. Per le selezioni su più tracce il movimento verticale conserva la spaziatura tra le tracce, con una sola modifica undoable.
- **Pan master**: i due pan stereo sul Master ora vengono applicati realmente al segnale, senza modificare le impostazioni di default hard-L/hard-R. Il surround non viene alterato.
- **Spot**: riprende la patch Core16 di posizione numerica, mentre Shuffle, Slip e Grid non vengono modificati.
- **Nuova icona**: immagine metallica color ambra preferita dall'utente, distribuita in PNG 512x512; build Mac genera `Sculpt DAW.icns`.

## Verifiche da eseguire dopo la build

1. Salva un progetto nuovo: deve essere `Prova/Prova.scdaw`, `Prova/Audio Files`. Riapri in una nuova esecuzione dell'app.
2. Apri un progetto `.scraft` preesistente: NON deve essere riscritto o spostato senza Save As esplicito.
3. Importa due WAV con **File > Import Audio**: deve aprirsi direttamente il browser, senza dialog intermedio.
4. Su 3 tracce audio, seleziona le tracce 1 e 2 e trascina un clip allineato: entrambe le clip devono seguire lo stesso delta e restare su tracce separate. Prova anche lo spostamento verticale del gruppo e Undo.
5. Seleziona due clip non allineate esplicitamente: spostale e verifica che offset e crossfade rimangano coerenti.
6. Su Master stereo, cambia entrambi i pan: l'audio deve modificarsi coerentemente. Ripristina -100/+100: suono identico all'originale.
7. Apri Memory Locations, crea un marker, rinominalo, richiamalo, salva e riapri: posizione/nome conservati. Verifica next/previous.
8. Apri un Send: deve esserci un solo controllo di livello interattivo; Pre/Post, Pan e Mute persistono.
9. Confronta meter Edit e Mix. Verifica VST3, AU, Return, fades, Shuffle/Slip/Grid, Spot, chiusura macOS.
10. Finder: doppio clic `.scdaw` da verificare **separatamente**; dichiarazione UTI non garantisce da sola la gestione dell'evento Apple Open Documents.

**Nota:** sono state verificate localmente sintassi Python, YAML, integrità del pacchetto e riferimenti alle patch. Non è stata ancora eseguita una build macOS della Core #17: i nuovi comportamenti non vanno dichiarati collaudati finché non passano CI e test in GUI/audio.
