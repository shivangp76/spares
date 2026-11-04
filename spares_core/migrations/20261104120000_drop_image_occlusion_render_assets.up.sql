-- Rendered image occlusions were stored as render assets under `@image-occlusions/<file name>`,
-- for Typst card sources that read them. Typst cards now have their clozes in their source, over
-- the original image, so no source reads them. Their stored copies are removed the next time a
-- render asset changes (`remove_unreferenced_render_assets`).
DELETE FROM render_asset WHERE path LIKE '@image-occlusions/%';
