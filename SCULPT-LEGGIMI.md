# Sculpt DAW 0.4 — sorgenti completi e compilazione macOS

Questo archivio contiene i sorgenti SoundCraft v0.6.1 (commit
6cf896e76c0ec82a11bf2494f209cfe7c8507f74) con le modifiche Sculpt.
Non contiene ancora un'app compilata. La compilazione macOS non è stata eseguita.

## GitHub

Caricare il contenuto in un repository dedicato, includendo `.github`.
Il workflow `Sculpt DAW - Mac Universal` parte con un push su main,
oppure da Actions → Sculpt DAW - Mac Universal → Run workflow.
Al termine, se verde, scaricare l'artifact Sculpt-DAW-Mac-Universal.
Al suo interno lo ZIP contiene Sculpt DAW.app per Apple Silicon e Intel.
Il workflow non pubblica release e non richiede certificati Apple.

## Compilazione sul Mac

Servono Xcode Command Line Tools e Rust stable (almeno 1.95).
Dalla cartella del progetto eseguire: `bash sculpt/build-macos.sh`.
Sono necessari internet e spazio per dipendenze e compilazione.
Risultato previsto: `dist/sculpt/Sculpt_DAW_0.4.0_Mac_Universal.zip`.

## Stato della verifica

Patch applicata al codice upstream reale; metadata e script controllati localmente.
App e CLI vengono compilate per entrambe le architetture; la procedura controlla
architetture e firma ad-hoc prima di produrre lo ZIP. Non notarizzata.
Avvio GUI, registrazione e caricamento plugin devono essere verificati su Mac.
La base SoundCraft è pre-alpha: questo pacchetto non la rende una DAW professionale completa.
L'importazione REAPER resta uno strumento esterno sperimentale in sculpt/compatibility.
La compatibilità completa con sessioni Pro Tools, Logic e Cubase non è implementata.

Gli altri workflow upstream non sono inclusi nell'archivio di consegna, per evitare
build e pubblicazioni non pertinenti. Licenze e attribuzioni originali sono conservate.
