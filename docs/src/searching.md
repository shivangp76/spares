# Searching

| Field               | Type     |
|---------------------|----------|
| id                  | i64      |
| data                | String   |
| created_at          | DateTime |
| updated_at          | DateTime |
| parser_name         | String   |
| tag                 | String   |
| keyword             | String   |
| custom_data         | Json     |
| c.id                | i64      |
| c.created_at        | DateTime |
| c.updated_at        | DateTime |
| c.stability         | f64      |
| c.difficulty        | f64      |
| c.desired_retention | f64      |
| c.suspended         | bool     |
| c.user_buried       | bool     |
| c.scheduler_buried  | bool     |
| c.state             | u32      |
| c.custom_data       | Json     |
| linked_to           | i64      |
| c.rated             | u32      |

## Types

### Strings

**Unquoted Strings:**
- Strings containing only alphanumeric characters (`a-z`, `A-Z`, `0-9`) do not need quotes.

**Quoted Strings:**
- Strings with non-alphanumeric characters must be quoted.
- Quotes can be escaped with a backslash.
- Alternatively, use `#"` and `"#` to delimit strings, where double quotes inside do not require escaping.

#### Tilde Operator (`~`)
- `field~"value"`: Searches if `field` contains `value`.
  - Example: `tag~"math test"` finds notes with tags containing `math test`.

#### Exact Match (`=`)
- `field="value"`: Searches for notes where `field` matches `value` exactly and `value` appears in the note's body.
  - Example: `tag="math test"` finds notes tagged `math test` with `test` in the body.

#### Default Field
- If no field is specified, `data` is used by default.
  - Example: `-dog` or `-"dog"` finds notes not containing `dog`.
  - Example: `"-dog"` finds notes containing `-dog`. This follows the quoting rule for non-alphanumeric characters like `-`.

### Numbers

Supported Rust types:
- `i64`
- `f64`
- `u32`

**Operators:**
- `=`
- `>`
- `>=`
- `<`
- `<=`

**Example:**
- `id>=5`

### Booleans

**Example:**
- `c.suspended=true`: Returns suspended cards.
- `c.suspended=false` or `-c.suspended=true`: Returns non-suspended cards.

### Dates

**Formats:**
- `YYYY-MM-DD`
- `YYYY-MM-DDTHH:MM:SSZ`

**Operators:**
- `=`
- `>`
- `>=`
- `<`
- `<=`

### JSON

JSON data can be queried using [JSONPath syntax](https://jsonpath.com/). The query result must be a boolean, number, or string. Use corresponding operators to filter results.

**Example JSON:**
```json
{
    "x": {
        "y": ["z", "zz"]
    },
    "a": {
        "b": false
    },
    "array": [
        {
            "key": 1
        }
    ]
}
```

**Examples:**
- `custom_data:"$.x.y[1]"=zz`
- `-custom_data:"$.a.b"`
- `custom_data:"$.array[0].key">=1`

### Other Operators

- **Exclusion:** `-QUALIFIER`
  - Example: `-tag=math`
- **Logical Operators:** `and`, `or`
- **Grouping:** Use parentheses for grouping expressions.

## Examples

**Search for notes containing "dog"**
- `dog`

**Search for notes containing "multiword \"query" with an escaped quote inside**
- `"multiword \"query"`

**Search for notes NOT containing "dog"**
- `-dog`

**Search for notes containing both "dog" AND "cat"**
- `dog and cat`

**Search for notes containing "dog" OR "cat"**
- `dog or cat`

**Search for "dog" AND either "cat" OR "mouse"**
- `dog and (cat or mouse)`

**Search for notes with the tag "math"**
- `tag=math`

**Search for notes with the tag "-math"**
- `tag=-math`

**Search for notes NOT tagged "math"**
- `-tag=math`

**Search for cards with stability ≥ 2**
- `c.stability>=2`

### Equivalences
- `dog` is equivalent to `data=dog` and `data="dog"`.
- `-cat -mouse` is equivalent to `-(cat or mouse)` (De Morgan's Laws).
- `dog cat` is equivalent to `dog and cat`. A space between terms implies an `and` operator unless part of a quoted string.

## Inspiration
- <https://github.com/github/docs/blob/main/content/search-github/getting-started-with-searching-on-github/understanding-the-search-syntax.md>
- <https://support.zendesk.com/hc/en-us/articles/4408835086106-Using-Zendesk-Support-advanced-search>
- <https://support.atlassian.com/trello/docs/searching-for-cards-all-boards/>
- <https://docs.ankiweb.net/searching.html>
