# The vendored font

`Caladea-Regular.ttf`, copyright (c) 2012 Huerta Tipografia, **licensed under the Apache License,
Version 2.0** -- the text is in `LICENSE` beside it; keep it with any copy. The licence is not taken
on trust from a distribution's packaging: the font's own `name` table carries `Licensed under the
Apache License, Version 2.0` and `http://www.apache.org/licenses/LICENSE-2.0`, which is what was
checked.

## Why a font is in this repository

**So that `curly gui` works on a machine with no fonts installed where this program can find them.**
`lib/graphics` has no notion of a system font and says so: there is no library to resolve a name
against, so a caller hands it bytes. A desktop application that asked the user to find a TrueType
file before it would open is not standalone, so one travels with it.

`gui/app.zm` still prefers a system face when it can see one, and `CURLY_FONT` overrides everything
-- this is the floor, not the preference. **It is a serif**, which is not the obvious choice for a
user interface; it is here because it was already licence-checked for ziman's own fixtures and
because the face is a one-line change once a sans with equally clear terms is picked.

## Why Apache 2.0 rather than the usual choice

The SIL Open Font Licence, which most freely available faces use, reserves the family name and
forbids a modified copy from keeping it -- which would matter if this were ever subsetted to shrink
the download. Apache 2.0 has no such clause.
