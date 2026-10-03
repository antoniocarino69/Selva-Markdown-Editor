# Piano: modalità WYSIWYG per Selva (Rust puro, egui)

Stato: implementazione di base integrata, validazione e completamento in corso.
Nessuna dipendenza JS/Electron: tutto in egui/eframe.

## Stato verificato (2026-10-02)

I sette moduli `src/editor/` e la modalità `DisplayMode::Wysiwyg` sono presenti.
Il modello attuale usa un parser per righe, non pulldown-cmark; mantiene il testo
originale e raggruppa fence e tabelle. Gli undo sono snapshot completi con limite
di 300 passi, non snapshot dei soli blocchi modificati.

- M0: round-trip byte-identical coperto da test unitari; manca la verifica su un
  corpus di vault e la copertura delle strutture Markdown non riconosciute.
- M1–M4: rendering, editing, stili inline, liste, code e immagini implementati
  nella versione di base. La virtualizzazione limita il painting, ma il primo
  layout attraversa ancora tutti i blocchi. Mancano golden screenshot e benchmark.
- M5: undo/redo, clipboard, selezione e gestione eventi di composizione presenti;
  drag multi-blocco e commit/annullamento IME con Unicode coperti da test headless.
  Restano da verificare IME reale e budget di memoria degli undo.
- M6: switch e persistenza integrati; le tabelle mostrano una griglia ma si
  modificano nel sorgente, senza editor per cella. Il completamento resta aperto.

La review ha corretto il payload delle immagini (PNG codificato per `from_bytes`),
la deformazione delle immagini in pannelli stretti, il panic di PageUp/PageDown
su note vuote e il coalescing undo dopo movimento del cursore. Il caret/IME viene
ora calcolato solo sul blocco del cursore, evitando che il blocco dell'anchor
sovrascriva la posizione. Il caricamento di una nota azzera il flag dirty.
Test aggiunti: decodifica e fedeltà dei pixel, undo dopo navigazione e navigazione
su documento vuoto tramite eventi egui headless.

La continuazione ha corretto la navigazione verticale: la colonna desiderata
viene conservata anche dentro un blocco, passando per righe corte o avvolte.
I target vengono scelti per riga visiva, evitando stime basate sull'altezza
della prima riga. Le mutazioni azzerano la colonna desiderata. Il layout viene
aggiornato dopo gli eventi di input e prima del painting: una sostituzione
multi-blocco non può più lasciare indici di cache riferiti al vecchio documento.
Test headless aggiunti per navigazione su codice e righe avvolte, composizione
Unicode con selezione e undo, annullamento IME e drag con copia/sostituzione.
Suite verificata: 83 test passati, un benchmark ignorato.

Prossimi passi: editing delle celle, layout virtualizzato e verifica IME reale.

## 0. Principio architetturale

**Il markdown su disco resta la fonte di verità.** Nessun formato proprietario: il modello
di editing è solo una cache strutturata del testo del file. Serializzazione = riscrittura
per blocchi, dove i blocchi non toccati mantengono i byte originali (front matter,
whitespace, sintassi strana inclusi). Questo elimina il rischio di corrompere i vault
dell'utente e mantiene la compatibilità Obsidian.

## 1. Architettura a moduli

Oggi `text_editor()` chiama `egui::TextEdit` — il WYSIWYG non può basarsi su `TextEdit`
(widget a stile singolo: un solo `FontId` per galley, cursori legati al testo grezzo).
Serve un editor a blocchi custom, ispirato a Typora/Obsidian:

```
src/editor/
  mod.rs        EditorWidget — stato di cursor/selection, ciclo di vita egui
  doc.rs        Doc = Vec<Block>; Block = { kind, source: String, dirty }
  inline.rs     parsing inline (bold/italic/code/link/strikethrough) → run con span sorgente
  layout.rs     block → LayoutJob/galley + mappa offset-sorgente ↔ posizioni cursore
  blocks.rs     rendering per tipo: heading, paragrafo, lista, quote, code, image, table, hr
  input.rs      tastiera/mouse/IME/clipboard → edit ops
  undo.rs       undo/redo con coalescing
```

## 2. Modello del documento (`doc.rs`)

- Parsing iniziale con **pulldown-cmark** (lo usa già `egui_commonmark` internamente,
  quindi zero dipendenze nuove): gli eventi espongono `Range<usize>`, quindi ogni blocco
  conosce il suo span nel sorgente.
- `Block { kind: BlockKind, text: String (frammento markdown esatto), inlines: Vec<InlineRun>, dirty: bool }`.
- **Round-trip lossless per costruzione**: il testo del file = concatenazione dei `text`
  dei blocchi. Un blocco viene riparsato (inlines) solo quando modificato. Modifiche fuori
  dall'editor (file esterno) → re-parse completo, con lo stesso check `saved_text` già
  usato in `save_current_file()`.
- Blocchi annidati (liste, blockquote) come blocchi con `indent: u8` piuttosto che albero
  profondo: più semplice da splittare/mergiare e copre il 95% dei casi reali.

## 3. Rendering (`layout.rs` + `blocks.rs`)

Ogni blocco è un widget egui che disegna la sua galley costruita a mano con `LayoutJob`:

- **Run con mappa**: ogni sezione del `LayoutJob` porta lo span sorgente `[start, end)`;
  la galley risultante mappa cluster-di-glyph → offset sorgente. Questa mappa è il cuore
  di tutto: cursore, selezione, click-to-position la leggono.
- **Sintassi nascosta**: `**`, `*`, `` ` ``, `#`, `>` nei limiti del blocco si rendono
  come run a larghezza zero o con `Color32::TRANSPARENT` e il cursore li salta nel
  movimento orizzontale (ma restano raggiungibili con le frecce da dentro la parola,
  comportamento Typora). I marker restano nel testo sorgente del blocco → salvataggio banale.
- **Heading**: `FontId::proportional(size)` per livello (28/24/20/18/16), il `# ` è nascosto.
- **Paragrafo**: galley proporzionale 18pt (allineato a `text_editor` attuale).
- **Lista**: bullet/numero dipinto nel gutter (riusa la logica del gutter righe già in
  `text_editor`), indentazione per livello; Tab/Shift+Tab cambiano livello.
- **Code block**: monospace + highlight **syntect** (già tra le dipendenze), riquadro con
  background `code_bg()`; si modifica in-place come testo monolinea-per-riga dentro il blocco.
- **Image**: `egui_extras::all_loaders` già caricato; risolvi il path relativo al vault con
  la stessa logica di `image_cache`/`save_pasted_image`.
- **Table**: griglia di sub-editor per cella; click in una cella = cursore lì dentro.
- **Quote**: barra laterale + indentazione, stile leggermente più debole.
- Virtualizzazione: `ScrollArea` + skip dei blocchi fuori viewport (stesso pattern di
  `show_rows` usato nel tree), con offset cumulativo stimato → scroll fluido anche su note
  da 2000 righe.

## 4. Input e editing (`input.rs`)

Stato: `cursor: (block: usize, offset: usize)`, `anchor` per la selezione
(multi-blocco come coppia (block, offset) start/end).

Edit ops atomiche (tutte registrate in undo):

- **Text insert** da `Event::Text` (gestire anche `Event::Ime`/composizione — egui 0.27 li
  passa: necessario per accenti/IME, è l'insidia classica degli editor custom).
- **Enter**: splitta il blocco al cursore; eredita il tipo (in lista continua il bullet, in
  code block va a capo *dentro* il blocco, in quote continua). Se il frammento nuovo è vuoto
  e il tipo è lista/quote → esce dal contesto (comportamento Notion).
- **Backspace all'inizio**: mergia col blocco precedente (join dei testi, il tipo del secondo
  viene convertito o azzerato), oppure outdent se la lista è annidata.
- **Canc/Frecce/Home/End/PageUp-Down**: con gestione shift-selezione e skip dei marker nascosti.
- **Ctrl+B/I/K/E** (bold/italic/link/code): wrap della selezione con marker markdown — la
  selezione multi-blocco wrappa parzialmente ogni blocco.
- **Ctrl+Z/Y**: undo stack in `undo.rs`, snapshot dei blocchi toccati con coalescing per
  parola (come fa `TextEdit`), profondità limitata in memoria.
- **Input rules** (caratteristica WYSIWYG a basso costo): digitare `# `, `## `, `> `,
  `- `, `1. `, ```` ``` ```` all'inizio di un blocco lo converte nel tipo corrispondente e
  nasconde il marker.
- **Clipboard**: copia = selezione serializzata in markdown grezzo (gli span li hai già);
  incolla = parsing del testo incollato in blocchi. Nota: `egui-winit` intercetta Ctrl+V
  prima di egui (pattern già gestito in `app.rs` per le immagini — riusare quel punto
  d'ingresso per strutturare il paste).

## 5. Integrazione in Selva (`app.rs`)

- `DisplayMode` diventa a 4 stati: `ViewOnly | EditAndPreview | EditOnly | Wysiwyg`; il
  toggle (Ctrl+Shift+V / comando palette) cicla e salva in `eframe::storage` come oggi.
- In modalità WYSIWYG la central panel renderizza `EditorWidget` invece di `text_editor()`;
  le altre modalità restano intatte (nessuna regressione possibile, il sorgente è sempre
  disponibile).
- Switch verso/da WYSIWYG = re-parse dal buffer `editor_text`; switch via = serializzazione
  back in `editor_text`, quindi `save_current_file()`, backlinks, tag, wiki-links, outline
  non cambiano di una riga — il WYSIWYG produce semplicemente il testo markdown.
- Outline/headings e backlinks continuano a usare `extract_headings()` sul testo
  serializzato (eventualmente alimentati dal modello per evitare il re-scan).

## 6. Milestone

| M | Contenuto | Criterio di uscita |
|---|-----------|-------------------|
| **M0** | `doc.rs` + parser blocchi + serializzazione lossless | test round-trip: `parse(serialize(parse(x))) == parse(x)` su un corpus di note reali (byte-identical sui blocchi non editati) |
| **M1** | Rendering read-only a blocchi (heading/para/lista/code/quote/immagine) + scroll virtualizzato | una nota complessa renderizza uguale alla preview `egui_commonmark`, test golden screenshot |
| **M2** | Cursore/selezione/click + typing + Enter/Backspace/frecce | test headless di interazione (pattern `pointer_frame` già nei test di `app.rs`): click-posizione, selezione shift-frecce, split/merge blocchi |
| **M3** | Inline styling, marker nascosti, input rules, Ctrl+B/I/K | toggle bold su selezione non rompe gli span; cursor skip dei marker |
| **M4** | Liste annidate + Tab/Outdent + code block editabile syntect + immagini | nesting fino a 3 livelli, code block con highlight |
| **M5** | Undo/redo, clipboard strutturato, IME, selezione multi-blocco | undo ripristina testo *e* cursore; composizione IME non perde caratteri |
| **M6** | Tabelle, polish (drag selection su più blocchi, drop di file), switch di modalità, persistenza | integrazione in `app.rs` + regressione completa della suite esistente |

Ordine di massima: M0-M2 danno un editor di testo già WYSIWYG per heading e paragrafi — da
lì il valore c'è già; M3-M4 sono ciò che lo rende "vero"; M5-M6 la completezza.

## 7. Test e rischi (come mitigarli)

- **Fidelità del round-trip** (il rischio #1): test di property su tutti i `.md` del vault —
  nessun byte cambia senza un edit esplicito. Questo gate protegge i dati utente più di
  qualsiasi altra cosa.
- **Mappa cursori/galley con marker nascosti**: isolata in `layout.rs` con test unitari puri
  (offset → posizione → offset round-trip) senza egui.
- **Performance su note grandi**: benchmark (c'è già `benchmark_vault_listing` come modello)
  su nota da 5k righe: parsing incrementale solo del blocco dirty, layout dei soli blocchi
  visibili.
- **Test headless di interazione**: obbligatori per click/selezione/drag secondo lo standard
  già adottato nel repo (harness `ctx.run(RawInput…)`), non solo test sulle edit-op.

## 8. Dipendenze

Nessuna nuova obbligatoria: `pulldown-cmark` (probabilmente già nell'albero tramite
`egui_commonmark`, da esplicitare in `Cargo.toml`), syntect, image, egui_extras già
presenti. Vincolo di portabilità (CRT statico, zero DLL extra) invariato — non entra nulla
di dinamico.
