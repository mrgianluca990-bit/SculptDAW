# Core28 — guasto dello stream di ingresso

Data: 19 ottobre 2026. Base: Core27 `f77519c`. Ambito: rilevazione errori input segnalati da CPAL, finalizzazione take, blocco REC e recovery esplicita. Nessuna modifica a SHOW o bridge.

## Verifiche automatiche

- `cargo test --locked -p soundcraft-playback --lib`: writer su disco con ingresso stereo sintetico, invocazione del callback errori effettivamente passato a CPAL con `DeviceNotAvailable`, confronto esatto dei campioni WAV, notifica singola e blocco riarmo prima della creazione file.
- `cargo test --locked -p soundcraft-ui-egui --lib input_stream_failure`: REC ferma il trasporto; playback senza REC continua; niente riapertura automatica.
- `cargo xtask ci`: gate completo, incluse regressioni mixer, I/O, plugin fixture e contratto bridge esistenti. Consultare la checklist del pacchetto per risultati effettivi e limitazioni dell'ambiente.

Il workflow macOS aggiunge il filtro delle nuove regressioni UI, compila lo SHA effettivo del branch anche in PR e condivide il gruppo di concorrenza tra push e PR dello stesso branch, evitando due build universal simultanee dello stesso lavoro. La verifica del merge con la base resta distinta dal gate sul commit distribuito.

Il test sintetico non scollega un dispositivo e non prova che ogni driver segnali ogni disconnessione. La risposta della UI avviene al frame successivo. Il test non certifica precisione di clock, latenza di registrazione o assenza di dropout.

## Installazione e collaudo sul Mac

1. Conservare l'app precedente e usare una copia del progetto; estrarre lo ZIP e installare la nuova app completa.
2. Aprire Playback Engine, selezionare input/output, rate e buffer compatibili; premere **Apply and reconnect audio**.
3. Assegnare ingressi hardware a due tracce mono o a una stereo; armare, registrare un segnale riconoscibile e fermare. Salvare e riaprire: verificare durata, canali e contenuto.
4. Su un progetto di prova avviare REC, poi scollegare l'interfaccia input. Se il driver segnala l'errore, verificare che REC si fermi, che la take acquisita resti nella sessione e sul disco, e che compaia l'avviso. Annotare se il driver non produce alcun evento: quel caso richiede diagnostica diversa.
5. Tentare un nuovo REC: deve essere rifiutato senza una nuova take vuota. Playback Engine deve indicare il guasto input.
6. Ricollegare, verificare la selezione, premere **Apply and reconnect audio**; provare una nuova registrazione, senza riavviare l'app. Se il trial fallisce, deve rimanere un errore esplicito.
7. Con input e output su dispositivi distinti, riprodurre clip senza REC e scollegare solo l'input: il playback deve continuare, l'ingresso deve risultare guasto. La disconnessione dell'unico dispositivo duplex può interrompere anche l'output: questo test non promette recovery output.
8. Regressioni: punch e loop record, monitoring, I/O nominati e Save/Undo; plugin reali e stato save/reopen; su copie dei progetti provare il round-trip SHOW→DAW→SHOW e verificare ordine stem e marker.

## Limiti mantenuti

Nessuno streaming delle sorgenti da disco introdotto; nessuna eliminazione della SRC DAW; nessuna compensazione clock/offset REC; nessun timeout per driver che smettono di chiamare senza notificare errori. Errori del writer e sincronizzazione stop/callback richiedono regressioni separate. Nessun cambio automatico di interfaccia durante la take.
