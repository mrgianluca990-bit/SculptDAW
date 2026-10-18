# Sculpt DAW Core25: backport mirati da SoundCraft 0.8.0

Data: 18 ottobre 2026. Base Sculpt invariata: SoundCraft 0.6.1; release corrente upstream esaminata: 0.8.0 (tag 2578ea6).

Questa branch NON sostituisce il core con la versione 0.8.0. Integra due correzioni isolate, presenti nella nuova linea SoundCraft, mantenendo Core20–24, il routing hardware a 64 canali e il bridge con Show.

## Implementato
1. **VST3 state** (upstream PR #400): sincronizza sul controller i parametri impostati dall'host prima di salvare, senza chiamate COM dall'audio thread. Un test fixture verifica il round-trip controller-only.
2. **PDC master** (upstream PR #409): la latenza degli insert sul Master viene contabilizzata nel bounce; è incluso il test originale upstream con un Maximizer.
3. **Build**: Core25 entra dopo Core24; cargo check e due regressioni Rust sono obbligatorie prima del pacchetto universal.

Origine: https://github.com/storytold/soundcraft ; licenze MIT OR Apache-2.0 originali conservate.

## Non incluso
Nessuna importazione indiscriminata di codice 0.8.0. Restano da portare in blocchi testati: streaming bounce/disk, recovery, plugin safety, audio device handling, audio file path security. Quest'ultima modifica va verificata rispetto a Show e ai file media esterni al progetto prima dell'adozione.

## Gate di accettazione
- Successo delle patch Core20–Core25, cargo check e test Rust.
- Test su Mac con VST3 reale: edit parametri, salva, riapri e controlla valori; verifica anche i meter FabFilter, che NON vengono corretti da Core25.
- Test PDC: bounce con Maximizer su Master, allineamento del transiente, bus/aux.
- Regressioni: Edit/Mix, routing 1–64, Finder, import 44.1/88.2→48, gruppi, cue Show → DAW, outbox Publish.
- La nuova build NON va promossa come stabile senza tutti i gate verdi e verifica macOS effettiva.

## Limiti
Il ricevitore Show per Apply Publish non è ancora implementato. Core24 può ancora fallire in compilazione o applicazione: Core25 non nasconde gli errori precedenti. La versione del binario resta 0.4.0 fino alla promozione verificata.
