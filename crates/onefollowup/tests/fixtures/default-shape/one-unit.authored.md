<!-- The one-unit form of the shipped default's ticket as its verifying agent authors it: the
     plain parts, the impact values in the record's `impact` (Severity: high; Workaround: read
     the listing twice and merge the pages; Severity with the workaround: low), no block and
     no estimate line. -->
## Root cause

A listing returns every page, however its count changes while it is read.

## Impact

**Who is affected.** Readers of the listing, and the export built on it.
**What it costs them.** A reader misses the last page of a listing whose count changed.
**What it costs the product owner.** One capability fails in some situations: a reader misses a page.
**Cost of the workaround.**
- Applying it: a reader adds one request per listing.
- Side effects on the outcome: none; the listing is complete.
- Discovery: free, because the failure names the workaround.

## Examples

A listing of 51 items read while one was added returned 50.

## Suggested fix

**Contributing locations.**
- some-service `src/cursor.py`: stops one page early when the count changes.

### Listing — `some-service` (`src/cursor.py`)

`list` pages by the cursor the previous page returned, so no page is skipped when the count changes.

## Evidence

A nightly export of the widgets listing came back one page short twice this week, each time while items were being added.
