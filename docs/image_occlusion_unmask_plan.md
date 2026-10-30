# Plan: see an image occlusion without its masks

On the back of a card, a button shows each of the card's image occlusions without masks: the original image and its Markup layer, with no clozes. This is the "Toggle Masks" button of Anki's Image Occlusion Enhanced add-on (`~/project_archive/image-occlusion-enhanced`, `template.py`), the one thing in that add-on spares lacks. It is for checking what the other masks hide once the card is answered, without editing the note's settings to reveal the whole note.

Today a card's image occlusion is drawn into a PNG, which the card names like any image:

- The server renders one PNG per occlusion and side (`get_image_occlusion_card_filepath`, `…-io-<n>.png` in `get_image_occlusion_rendered_directory`).
- A device makes its PNGs when asked, named by what they're made from (`image_occlusion/device.rs`).
- Typst cards instead show an SVG of the clozes over the original image (`image_occlusion_overlay_svg`, `Parseable::image_occlusion_overlays`).
- The card that holds the image is a PDF (LaTeX, `PdfViewer`), HTML (Markdown, `HtmlViewer`) or compiled in the browser (Typst, `TypstViewer`).

## Interface

On the back, when the card has an image occlusion with something still masked, the review toolbar gets a button: **Without masks** (key `m`, listed in the `?` popup). It opens an overlay above the card with each of the card's occlusions unmasked, in the note's order, captioned like the card's figure. Pressing the button or `m` again, Escape, or clicking outside it closes the overlay. Rating or leaving the card closes it too, so the next card starts with its masks.

```
┌ review toolbar ─────────────── [Without masks] ┐
│ ┌───────────── overlay ──────────────────────┐ │
│ │  Brain                                  ✕  │ │
│ │  (original image + markup, no clozes)      │ │
│ └────────────────────────────────────────────┘ │
│ (card, behind)                                 │
└────────────────────────────────────────────────┘
```

- **No button on the front.** It would give the answer away.
- **No button when nothing is masked on the back** (`BackReveal::FullNote` without emphasis, and no "don't answer" masks left). The image would be the same as the card's.
- **An overlay, not swapping the image in the card.** Only Markdown cards are HTML whose `<img>` could be swapped. A PDF can't be changed, and a Typst card would have to be recompiled. The overlay works the same for all three, and the card stays as it was rendered.
- **A toolbar button, not tapping the image.** Tapping the front already shows the answer (`ReviewPage.tsx`), and a tap on a PDF or Typst page can't tell where the image is.
- **`m` is free:** the review keys are Space, digits, `u U b B l d s e R f q Q r w + = - ?`.

## Cost

The unmasked image is rarely looked at, so nothing is made for it until the button is pressed. Showing a card does only what it does today, plus a check on data it already has. Pressing the button reads the original image and, if the occlusion has markup, builds a small SVG. No image is decoded or encoded, and nothing is written to disk.

## 1. The unmasked image

It is the original image, with the Markup layer drawn over it when that layer has shapes. That's two parts, like a Typst card's image occlusion, never one composed PNG:

- **The image:** the original image's bytes, as they are.
- **The markup:** `unmasked_overlay_svg(image_occlusion_data, original_image) -> Option<Vec<u8>>` in `construct.rs`. It reads the clozes SVG, empties its Clozes layer (`get_clozes_from_svg`, then clears the group's children) and wraps it with `image_occlusion_overlay_svg`, which reads only the image's header for its size. It is `None` when the Markup layer has no shapes, which is the common case, so the overlay shows the original image alone.

The overlay stacks the two at the same width, as Typst cards do. So the server never runs `compose_image_occlusion_card` for this, and neither does a device in wasm. That function's PNG encode is the costly step, and a PNG of a photo is often several times bigger than the original JPEG. The markup SVG is a few KB.

## 2. Telling the review page

`GetReviewCardResponse` (`schema.rs`) gets `image_occlusions: Option<Vec<UnmaskedImageOcclusion>>`, set in `add_card_extras` (`review.rs`) next to `typed_answer` and `audio`:

```rust
pub struct UnmaskedImageOcclusion {
    pub caption: String, // as the card's figure, `image_occlusion_caption`
    pub order: usize,    // the occlusion's place in the card, from 1, as `-io-<n>`
}
```

It lists the card's occlusions that still mask something on the back (section "Interface"), and is `None` when there are none, so the button shows only when it would change something. It is made from the card that `load_review_card` has already parsed. The shown masks come from the cloze indices and settings that `modify_clozes_for_card` already gets, so the check goes beside it as `masks_on_back(...) -> bool`. It reads no files and names no image, so a review costs nothing more when the button isn't used.

## 3. Serving it

`GET /api/review/unmasked-image-occlusion?note_id=…&card_order=…&order=…&part=image|markup` (`spares_server/src/route.rs`, with a core function in `api/review.rs`). It loads the card as `load_review_card` does, finds the occlusion and answers:

- `part=image`: the original image's bytes, with its media type.
- `part=markup`: the SVG from section 1, or `204 No Content` when there is no markup.

The occlusion is found by its note and card, never by a path the client sends, so nothing outside the collection can be asked for.

- **Not render assets.** `/api/render-assets` serves only the `render_asset` rows that `sync_render_assets` (`browser_render.rs`) stores: Typst sources' dependencies and audio clips. A LaTeX or Markdown card's original image isn't one of them, and storing every original there would upload another copy of each.
- **Caching:** an `ETag` made from the hash of the original image's path, size and modification time, plus the markup SVG, with `Cache-Control: private, no-cache`. Opening the overlay again gets a `304`.
- **Device:** a device that keeps a copy of the collection answers with the same core function, compiled to wasm. It reads the original image with `crate::platform::read`, as `device::read_card` does. Its client checks `usesLocalCopy` before asking the server, as `fetchRenderAsset` does. Building the SVG is an XML edit, so it's quick in wasm too. `device.rs` needs no new variant, and the sync needs no change.

## 4. Frontend

- `types/spares.ts`: the new field.
- `api/client.ts`: `fetchUnmaskedImageOcclusion(noteId, cardOrder, order)`. It fetches both parts in parallel and returns object URLs for the image and, if there is one, the markup.
- `ReviewPage.tsx`: the button and `m` binding, available when `phase === 'back'` and the field is non-empty; `Escape` closes the overlay. It fetches when the overlay opens, never before.
- `components/UnmaskedImageOcclusions.tsx`: the overlay. For each occlusion it shows the image with the markup `<img>` absolutely positioned over it at the same width, and fits them to the viewport. It keeps the card zoom setting out, since the image is shown at its own size. It revokes its object URLs when it closes.

## Tests

- `construct.rs`: `unmasked_overlay_svg` keeps the Markup layer and empties the Clozes layer, is the original image's size, and is `None` when the Markup layer is empty.
- `review.rs`: the field is set for a back with masks left, and `None` on the front, on a back that reveals the full note, and for cards without occlusions. The core function returns the original image, the markup SVG or none, and not found for an `order` past the card's occlusions.
- `spares_server`: the route returns `304` for a matching `If-None-Match`.
- Playwright (e2e): on the back, `m` opens the overlay with the image and its markup, and the next card opens without it.

## Not doing

- **A composed PNG per occlusion**, rendered with the card's PNGs. It would cost a decode and a PNG encode on every note render (and in wasm on a device) for an image that's rarely opened. It would also leave a `<hash>.png` behind after every edit of the clozes that nothing deletes, and need adding to the render-asset sync before a web client could fetch it.
- **Toggling on the front**, and keeping the overlay open across cards.
- **Swapping the image inside Markdown cards.** It would work for one format of three and make the formats behave differently.
- **Hiding only some masks**, e.g. clicking a mask to lift it. Anki's add-on doesn't either, and it's more interface than the need.
