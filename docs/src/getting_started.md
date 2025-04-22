# Getting Started

Clone this repository:
```sh
git clone https://github.com/shivangp76/spares.git
cd spares
cargo build --release
export PATH="$PATH:~/spares/target/release"
```

## Creating a parser (optional)

Spares ships with 2 main parsers: "markdown" and "latex-note".

The "markdown" parser serves as a guide for a basic parser, while the "latex-note" parser is more advanced. If you are creating your own parser, it is probably best to copy one of these as your starting template. The full specifications of these parsers can be found in the code documentation.

Note that creating or modifying the parsers will require recompiling the package.

## Starting the server

Run the following command in the terminal:
```sh
spares_server
```

### Migration (optional)

Spares ships with a script to migrate data from Anki. See `spares_migrate --help`.

## Adding notes

Spares ships with a CLI to interact with the server. Its documentation can be found by running `spares_cli --help`.

Using the CLI, we can add our parsers to the database. This is needed for creating notes that use these parsers.
```sh
spares_cli add parser --name markdown
spares_cli add parser --name latex-note
```

The CLI also provides an import functionality to add notes in bulk. For example, using the "markdown" parser, we can create a file called `notes.md` with the following contents:
```md
<!--- spares: start --->

<!--- # tags: geology, oceans --->
<!--- spares: note start --->
# Oceans

- The 5 oceans in the world are:
    - {{ [Pacific](li) }}
    - {{ Atlantic }}
    - {{ Indian }}
    - {{ Southern }}
    - {{ Arctic }}
<!--- spares: note end --->

<!--- # tags: geology, oceans --->
<!--- # keywords: Pacific Ocean --->
<!--- spares: note start --->
# Pacific Ocean

- The deepest point in the Pacific Ocean is {{the Mariana Trench}}

<!--- spares: note end --->

<!--- spares: end --->
```

We can import this file with the following command:
```sh
spares_cli import --parser markdown ./notes.md
```
(See `spares_cli import --help` for more options)

## Linked Notes

Note the usage of `[Pacific](li)` in the first note. This is a linked note. The keyword "Pacific" will be searched for across all other notes and the note with the closest match will be linked in that note document. In this case, the second note has the keyword "Pacific Ocean". Since this is the only other note, this is the closest match. This linking will be done when notes are rendered (see below).

# Render notes

You can render notes with the following command. This will create the following files for each note:

- The note's text file.
- The note's rendered file. For the markdown parser, this is a pdf file.
- For each card:
    - The card's text file.
    - The card's rendered file. For the markdown parser, this is a pdf file.

```sh
spares_cli render --include-linked-notes --include-cards --render
```

The note's text file will also contain the linked notes. The exact syntax of these files can be modified in the parser.

## Editing notes

Notes can be edited by directly editing their corresponding file which is created after rendering. They can then be reimported in (see `spares_cli import --help`).

To facilitate importing multiple notes, you can sync notes between your local files and spares:
```sh
spares_cli sync
```
This will walk you through your changes, letting you decide what to keep and what to discard.

# Reviewing

```sh
spares_cli review
```

## Image Occlusions

Here is an example of an note using the "markdown" parser with an image occlusion:
```md
<!--- spares: start --->

<!--- spares: note start --->
# Brain

This is a note about the brain.

<!--- spares: image occlusion start --->
<!--- original_image_filepath = "/misc/image_occlusions/brain.jpg" --->
<!--- clozes_filepath = "/misc/image_occlusions/brain_clozes.jpg" --->
<!--- spares: image occlusion end --->

<!--- spares: note end --->

<!--- spares: end --->
```

To create an image occlusion:

1. Run the `spares_io` binary. This will automatically open the image occlusion editor in your web browser.
2. Click "Change Background Image" and select the image you want to create clozes for.
3. Add markup and clozes to the appropriate layer. You can add cloze settings strings to clozes as needed.
4. Click "Save SVG" to save your work.
5. In your note document, use the image occlusion snippet to insert the saved SVG.

The editor allows you to create multiple instances simultaneously, making it easy to work on different image occlusions at once. You can examine the generated SVG files to see exactly how the clozes are parsed. A more intuitive interface with `svg-edit` will be available in future updates.

## Audio/ Video

Refer to the `typst` parser and use `html` export.
