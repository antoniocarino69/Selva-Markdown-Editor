# Selva Theme Design System

Obsidian-inspired design tokens for the egui 0.27 rendering layer.  
Every token maps to a concrete `egui::Color32`, `egui::FontId`, or `egui::Vec2` so nothing is left to interpretation.

---

## 1  Color Palette

### 1.1  Base Scale (Neutral)

Based on Obsidian's `--color-base-*` series. These define the surface hierarchy from deepest background to brightest text.

| Token             | Role                          | Dark (hex) | Light (hex) |
|-------------------|-------------------------------|------------|-------------|
| `bg_base`         | Deepest background            | `#1e1e1e`  | `#ffffff`   |
| `bg_primary`      | Note editor canvas            | `#232323`  | `#fafafa`   |
| `bg_secondary`    | Sidebar / chrome              | `#1c1c1c`  | `#f6f6f6`   |
| `bg_elevated`     | Popups, menus, windows        | `#2e2e2e`  | `#efefef`   |
| `bg_hover`        | Hover row / faint stripe      | `#333333`  | `#e4e4e4`   |
| `bg_active`       | Active list item / selection  | `#3f3f3f`  | `#dadada`   |
| `border_subtle`   | Panel dividers, faint strokes | `#555555`  | `#bdbdbd`   |
| `border_strong`   | Focus rings, active outlines  | `#666666`  | `#ababab`   |
| `text_primary`    | Body text                     | `#dadada`  | `#222222`   |
| `text_secondary`  | Labels, muted text            | `#999999`  | `#707070`   |
| `text_faint`      | Placeholders, line numbers    | `#555555`  | `#bdbdbd`   |
| `text_on_accent`  | Text inside accent buttons    | `#ffffff`  | `#ffffff`   |

### 1.2  Accent & Semantic Colors

Derived from Obsidian's 8-color accent palette, tuned for WCAG AA contrast on both backgrounds.

| Token           | Role                            | Dark (hex) | Light (hex) |
|-----------------|---------------------------------|------------|-------------|
| `accent_blue`   | Primary accent, links, focus    | `#027aff`  | `#086ddd`   |
| `accent_purple` | Heading accent, tags, special   | `#a882ff`  | `#7852ee`   |
| `accent_cyan`   | Internal wiki-links             | `#53dfdd`  | `#00bfbc`   |
| `accent_green`  | Success, resolved links         | `#44cf6e`  | `#08b94e`   |
| `accent_orange` | Bold emphasis, warnings         | `#e9973f`  | `#ec7500`   |
| `accent_red`    | Errors, unresolved links        | `#fb464c`  | `#e93147`   |
| `accent_yellow` | Italic emphasis, callouts       | `#e0de71`  | `#e0ac00`   |
| `accent_pink`   | Tags, abstract references       | `#fa99cd`  | `#d53984`   |

### 1.3  Syntax Highlighting Tokens (for egui_commonmark / syntect)

| Token              | Role                        | Dark (hex) | Light (hex) |
|--------------------|-----------------------------|------------|-------------|
| `syn_keyword`      | function, return, import    | `#e9973f`  | `#ec7500`   |
| `syn_string`       | String literals             | `#44cf6e`  | `#08b94e`   |
| `syn_number`       | Numeric literals            | `#a882ff`  | `#7852ee`   |
| `syn_comment`      | Comments                    | `#666666`  | `#ababab`   |
| `syn_function`     | Function/method names       | `#e0de71`  | `#e0ac00`   |
| `syn_type`         | Types, classes              | `#53dfdd`  | `#00bfbc`   |
| `syn_variable`     | Variables, parameters       | `#dadada`  | `#222222`   |
| `syn_operator`     | Operators, punctuation      | `#fb464c`  | `#e93147`   |
| `syn_bg_code`      | Inline code background      | `#2e2e2e`  | `#efefef`   |
| `syn_bg_codeblock` | Code block background       | `#1e1e1e`  | `#f6f6f6`   |

### 1.4  Markdown Prose Accents

These tint **bold** and *italic* text in the editor, matching Obsidian's `--bold-color` / `--italic-color`.

| Token             | Usage          | Dark (hex) | Light (hex) |
|-------------------|----------------|------------|-------------|
| `prose_bold`      | **Bold text**  | `#fb464c`  | `#e93147`   |
| `prose_italic`    | *Italic text*  | `#e9973f`  | `#ec7500`   |
| `prose_heading`   | H1–H3 accent   | `#53dfdd`  | `#00bfbc`   |
| `prose_link`      | Wiki-links     | `#53dfdd`  | `#00bfbc`   |
| `prose_link_ext`  | External links | `#027aff`  | `#086ddd`   |
| `prose_link_broken`| Unresolved    | `#fb464c`  | `#e93147`   |
| `prose_tag`       | #tags          | `#fa99cd`  | `#d53984`   |

---

## 2  Typography

### 2.1  Font Families

| Token         | egui Role          | Recommended Stack                              |
|---------------|--------------------|-------------------------------------------------|
| `font_ui`     | UI chrome          | `"Segoe UI"`, `"SF Pro Text"`, `"Helvetica Neue"`, system sans |
| `font_body`   | Editor / prose     | `"Inter"`, `"Source Sans 3"`, system sans       |
| `font_mono`   | Code blocks, gutter| `"JetBrains Mono"`, `"Fira Code"`, `"Cascadia Code"`, monospace |

Load via `cc.egui_ctx.set_fonts(FontDefinitions)`:
- **Proportional** → `font_body` (editor) and `font_ui` (widgets — egui picks one family for all `TextStyle::Body`, override per widget with `RichText::family()`).
- **Monospace** → `font_mono`.

### 2.2  Type Scale

| Token          | egui TextStyle      | Size (px) | Weight  | Usage                       |
|----------------|---------------------|-----------|---------|-----------------------------|
| `text_h1`      | Heading             | 28        | 700     | Note title in editor        |
| `text_h2`      | Heading             | 24        | 700     | Section heading             |
| `text_h3`      | Heading             | 20        | 600     | Sub-heading                 |
| `text_body`    | Body                | 16        | 400     | Editor prose, labels        |
| `text_small`   | Small               | 13        | 400     | Status bar, metadata        |
| `text_tiny`    | Monospace (inline)  | 11        | 400     | Line numbers, debug         |
| `text_button`  | Button              | 14        | 500     | Button labels               |
| `text_mono`    | Monospace           | 15        | 400     | Code editor, inline code    |

Apply with `style.text_styles.insert(TextStyle::Body, FontId::new(16.0, FontFamily::Proportional))`.

### 2.3  Line Heights

| Context         | Multiplier | Pixel at 16px body |
|-----------------|------------|---------------------|
| Prose           | 1.5        | 24                  |
| Tight (UI list) | 1.3        | 21                  |
| Code            | 1.4        | 22                  |

egui handles line height internally via font metrics; these are visual targets for `RowHeight` in custom layouts.

---

## 3  Spacing System

8px base grid, following Obsidian's convention.

| Token            | Value | Usage                                    |
|------------------|-------|------------------------------------------|
| `space_xs`       | 4     | Icon padding, inline gaps                |
| `space_sm`       | 8     | Default item spacing (`item_spacing.y`)  |
| `space_md`       | 12    | Section padding inside panels            |
| `space_lg`       | 16    | Between major sections                   |
| `space_xl`       | 24    | Panel inner margin                       |
| `space_2xl`      | 32    | Vertical breathing room before headings  |

Map to `egui::style::Spacing`:

```rust
spacing.item_spacing      = vec2(8.0, 8.0);      // space_sm
spacing.button_padding    = vec2(8.0, 6.0);       // horizontal sm, vertical ~sm
spacing.window_margin     = Margin::same(12.0);    // space_md
spacing.indent            = 18.0;                   // tree indent
spacing.scroll            = ScrollStyle { ... };   // bar width = 6
spacing.window_margin     = Margin { left: 16, right: 16, top: 12, bottom: 12 };
```

---

## 4  Border Radius

Obsidian uses minimal rounding. Map to `egui::CornerRadius`.

| Token              | Value | Usage                             |
|--------------------|-------|-----------------------------------|
| `radius_none`      | 0     | Code blocks, full-bleed panels    |
| `radius_xs`        | 2     | Inline code background            |
| `radius_sm`        | 4     | Buttons, inputs, tags             |
| `radius_md`        | 6     | Cards, panels, popups             |
| `radius_lg`        | 10    | Windows, dialogs                  |
| `radius_full`      | 999   | Pill buttons, badges              |

Apply to `WidgetVisuals.corner_radius`, `window_corner_radius`, `menu_corner_radius`, and `Frame::rounding`.

---

## 5  Shadows & Depth

egui uses `Shadow { offset, blur, spread, color }`. Obsidian uses very subtle shadows.

| Token           | offset   | blur | spread | color (dark)          | color (light)         |
|-----------------|----------|------|--------|-----------------------|-----------------------|
| `shadow_sm`     | [0, 2]   | 4    | 0      | `rgba(0,0,0,0.25)`   | `rgba(0,0,0,0.08)`   |
| `shadow_md`     | [0, 6]   | 10   | 0      | `rgba(0,0,0,0.35)`   | `rgba(0,0,0,0.12)`   |
| `shadow_lg`     | [0, 12]  | 20   | 0      | `rgba(0,0,0,0.45)`   | `rgba(0,0,0,0.18)`   |
| `shadow_popup`  | [4, 6]   | 8    | 0      | `rgba(0,0,0,0.38)`   | `rgba(0,0,0,0.12)`   |

Apply to `Visuals.window_shadow`, `Visuals.popup_shadow`, or paint manually with `ui.painter().rect_filled(...)`.

---

## 6  Interactive States

### 6.1  Widget State Colors

Map to `egui::style::Widgets` — each state has `bg_fill`, `weak_bg_fill`, `bg_stroke`, `fg_stroke`, `corner_radius`, `expansion`.

| State          | bg_fill (dark)  | fg_stroke (dark)  | bg_fill (light)  | fg_stroke (light)  |
|----------------|-----------------|-------------------|------------------|--------------------|
| noninteractive | `bg_primary`    | `text_secondary`  | `bg_primary`     | `text_secondary`   |
| inactive       | `bg_hover`      | `text_primary`    | `bg_hover`       | `text_primary`     |
| hovered        | `bg_active`     | `text_primary`    | `bg_active`      | `text_primary`     |
| active         | `accent_blue`   | `text_on_accent`  | `accent_blue`    | `text_on_accent`   |

### 6.2  Selection

| Token              | Dark                          | Light                          |
|--------------------|-------------------------------|--------------------------------|
| `selection_bg`     | `rgba(2, 122, 255, 0.25)`     | `rgba(8, 109, 221, 0.20)`      |
| `selection_stroke` | `rgba(2, 122, 255, 0.50)`     | `rgba(8, 109, 221, 0.40)`      |

### 6.3  Focus Ring

| Token         | Color            | Width |
|---------------|------------------|-------|
| `focus_ring`  | `accent_blue`    | 2.0   |

Apply by painting a rounded rect stroke around focused widgets using `response.has_focus()`.

---

## 7  Panel & Surface Mapping

The app has distinct visual zones. Map tokens to `egui::Visuals` fields:

| UI Zone              | Background Token  | egui Field / Frame                        |
|----------------------|-------------------|-------------------------------------------|
| Sidebar              | `bg_secondary`    | `Visuals.panel_fill` (SidePanel)          |
| Editor canvas        | `bg_primary`      | `CentralPanel` background                 |
| Text editor field    | `bg_base`         | `Visuals.extreme_bg_color` / `text_edit_bg_color` |
| Status bar           | `bg_secondary`    | `TopBottomPanel::bottom`                  |
| Popup / context menu | `bg_elevated`     | `Visuals.window_fill`                     |
| Search box           | `bg_base`         | `Frame::fill` on search container         |
| Preview pane         | `bg_primary`      | Right `SidePanel` background              |
| Window / dialog      | `bg_elevated`     | `Visuals.window_fill`                     |
| Code block bg        | `syn_bg_codeblock`| `Visuals.code_bg_color`                   |

---

## 8  Hover & Tooltip Styling

| Token              | Value (dark)        | Value (light)       |
|--------------------|---------------------|---------------------|
| `tooltip_bg`       | `#333333`           | `#efefef`           |
| `tooltip_text`     | `text_primary`      | `text_primary`      |
| `tooltip_border`   | `#555555`           | `#bdbdbd`           |
| `tooltip_radius`   | `radius_md` (6)     | `radius_md` (6)     |
| `tooltip_shadow`   | `shadow_sm`         | `shadow_sm`         |

---

## 9  Implementation Map (egui 0.27 API)

### 9.1  Visuals

```rust
fn apply_visuals(v: &mut egui::Visuals, dark: bool) {
    v.dark_mode = dark;

    if dark {
        // Surfaces
        v.panel_fill        = Color32::from_rgb(28, 28, 28);    // bg_secondary
        v.window_fill       = Color32::from_rgb(46, 46, 46);    // bg_elevated
        v.extreme_bg_color  = Color32::from_rgb(30, 30, 30);    // bg_base
        v.faint_bg_color    = Color32::from_rgb(51, 51, 51);    // bg_hover
        v.code_bg_color     = Color32::from_rgb(30, 30, 30);    // syn_bg_codeblock
        v.text_edit_bg_color = Some(Color32::from_rgb(30, 30, 30));

        // Text
        v.override_text_color = Some(Color32::from_rgb(218, 218, 218)); // text_primary
        v.weak_text_alpha     = 0.55;

        // Semantic
        v.hyperlink_color  = Color32::from_rgb(2, 122, 255);    // accent_blue
        v.warn_fg_color    = Color32::from_rgb(233, 151, 63);   // accent_orange
        v.error_fg_color   = Color32::from_rgb(251, 70, 76);    // accent_red

        // Selection
        v.selection.bg_fill   = Color32::from_rgba_premultiplied(2, 122, 255, 64);
        v.selection.stroke    = Stroke::new(1.0, Color32::from_rgba_premultiplied(2, 122, 255, 128));

        // Borders
        v.window_stroke = Stroke::new(1.0, Color32::from_rgb(85, 85, 85)); // border_subtle

        // Corners
        v.window_corner_radius = CornerRadius::same(10);  // radius_lg
        v.menu_corner_radius   = CornerRadius::same(6);   // radius_md

        // Shadows
        v.window_shadow = Shadow { offset: [0, 6], blur: 10, spread: 0,
                                     color: Color32::from_black_alpha(89) };
        v.popup_shadow  = Shadow { offset: [4, 6], blur: 8, spread: 0,
                                     color: Color32::from_black_alpha(97) };
    } else {
        // Light mirror — same structure, light tokens
        v.panel_fill        = Color32::from_rgb(246, 246, 246);
        v.window_fill       = Color32::from_rgb(239, 239, 239);
        v.extreme_bg_color  = Color32::from_rgb(250, 250, 250);
        v.faint_bg_color    = Color32::from_rgb(228, 228, 228);
        v.code_bg_color     = Color32::from_rgb(246, 246, 246);
        v.text_edit_bg_color = Some(Color32::from_rgb(255, 255, 255));

        v.override_text_color = Some(Color32::from_rgb(34, 34, 34));
        v.weak_text_alpha     = 0.50;

        v.hyperlink_color  = Color32::from_rgb(8, 109, 221);
        v.warn_fg_color    = Color32::from_rgb(236, 117, 0);
        v.error_fg_color   = Color32::from_rgb(233, 49, 71);

        v.selection.bg_fill   = Color32::from_rgba_premultiplied(8, 109, 221, 51);
        v.selection.stroke    = Stroke::new(1.0, Color32::from_rgba_premultiplied(8, 109, 221, 102));

        v.window_stroke = Stroke::new(1.0, Color32::from_rgb(189, 189, 189));
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius   = CornerRadius::same(6);

        v.window_shadow = Shadow { offset: [0, 6], blur: 10, spread: 0,
                                     color: Color32::from_black_alpha(31) };
        v.popup_shadow  = Shadow { offset: [4, 6], blur: 8, spread: 0,
                                     color: Color32::from_black_alpha(31) };
    }
}
```

### 9.2  Widget States

```rust
fn apply_widgets(w: &mut egui::style::Widgets, dark: bool) {
    // Noninteractive: passive labels, separators
    w.noninteractive.bg_fill    = Color32::TRANSPARENT;
    w.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    w.noninteractive.fg_stroke  = Stroke::new(1.0, if dark { Color32::from_rgb(153,153,153) } else { Color32::from_rgb(112,112,112) });
    w.noninteractive.corner_radius = CornerRadius::same(4);

    // Inactive: resting buttons, tabs
    w.inactive.bg_fill     = Color32::TRANSPARENT;
    w.inactive.weak_bg_fill = if dark { Color32::from_rgb(46,46,46) } else { Color32::from_rgb(228,228,228) };
    w.inactive.fg_stroke   = Stroke::new(1.0, if dark { Color32::from_rgb(218,218,218) } else { Color32::from_rgb(34,34,34) });
    w.inactive.corner_radius = CornerRadius::same(4);

    // Hovered
    w.hovered.bg_fill     = if dark { Color32::from_rgb(63,63,63) } else { Color32::from_rgb(228,228,228) };
    w.hovered.weak_bg_fill = w.hovered.bg_fill;
    w.hovered.fg_stroke   = w.inactive.fg_stroke;
    w.hovered.corner_radius = CornerRadius::same(4);
    w.hovered.expansion   = 1.0;

    // Active: pressed button
    w.active.bg_fill     = Color32::from_rgb(2, 122, 255); // accent_blue
    w.active.weak_bg_fill = w.active.bg_fill;
    w.active.fg_stroke   = Stroke::new(1.0, Color32::WHITE);
    w.active.corner_radius = CornerRadius::same(4);
    w.active.expansion   = 2.0;
}
```

### 9.3  Typography

```rust
fn apply_typography(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.text_styles.insert(TextStyle::Heading,   FontId::new(28.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Body,      FontId::new(16.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Monospace,  FontId::new(15.0, FontFamily::Monospace));
    style.text_styles.insert(TextStyle::Button,     FontId::new(14.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Small,      FontId::new(13.0, FontFamily::Proportional));

    ctx.set_style(style);
}
```

---

## 10  Applying to the App

Replace the current `apply_theme` in `app.rs` with a version that:

1. Creates `Visuals::dark()` or `Visuals::light()` as base.
2. Calls `apply_visuals(&mut visuals, dark)` to set all surface/semantic tokens.
3. Calls `apply_widgets(&mut visuals.widgets, dark)` for interactive states.
4. Sets `ctx.set_visuals(visuals)`.
5. Calls `apply_typography(ctx)` for the type scale.
6. Optionally loads custom fonts with `FontDefinitions` for `font_body` / `font_mono` / `font_ui`.

### 10.1  Additional RichText Usage

For accent-colored text in the UI (not covered by Visuals), use `egui::RichText`:

```rust
// Heading with accent color
ui.label(RichText::new(title).size(28.0).color(Color32::from_rgb(83, 223, 221)));

// Wiki-link styling
let link_text = RichText::new(link).color(Color32::from_rgb(83, 223, 221)).underline();

// Unresolved link
let broken = RichText::new(link).color(Color32::from_rgb(251, 70, 76)).strikethrough();

// Tag
let tag = RichText::new("#topic").size(13.0).color(Color32::from_rgb(250, 153, 205));
```

### 10.2  Frame Styling

For custom card/surface backgrounds:

```rust
egui::Frame::none()
    .fill(Color32::from_rgb(46, 46, 46))          // bg_elevated
    .stroke(Stroke::new(1.0, Color32::from_rgb(85, 85, 85)))  // border_subtle
    .rounding(CornerRadius::same(6))               // radius_md
    .inner_margin(Margin::same(12))                // space_md
    .outer_margin(Margin::same(8))                 // space_sm
    .shadow(Shadow { offset: [0, 2], blur: 4, spread: 0, color: Color32::from_black_alpha(64) })
    .show(ui, |ui| { /* content */ });
```

---

## 11  Future: User-Configurable Themes

The token table in §1.1–§1.4 can be serialized as a `serde` struct:

```rust
#[derive(Serialize, Deserialize)]
struct ThemeTokens {
    bg_base: Color32,
    bg_primary: Color32,
    // ... all tokens from §1
}
```

Store in `eframe::Storage` or a TOML/JSON file. The `apply_visuals` function would accept `&ThemeTokens` instead of a boolean, enabling custom palettes or community themes.

---

## References

- [Obsidian CSS Variables — Colors](https://docs.obsidian.md/Reference/CSS+variables/Foundations/Colors)
- [Obsidian CSS Variables — Typography](https://docs.obsidian.md/Reference/CSS+variables/Foundations/Typography)
- [egui 0.27 Visuals](https://docs.rs/egui/0.27/egui/style/struct.Visuals.html)
- [egui 0.27 Style](https://docs.rs/egui/0.27/egui/style/struct.Style.html)
- [egui 0.27 WidgetVisuals](https://docs.rs/egui/0.27/egui/style/struct.WidgetVisuals.html)
- [Forest Phosphor — Obsidian Theme](https://forestphosphor.dev/obsidian)
- [Dracula — Obsidian Theme](https://github.com/dracula/obsidian)