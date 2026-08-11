# Storage format

The storage format is deterministic, versioned, printable, and one line long. A representative database looks like this:

```text
V2;~S|users|id:I:!|name:T:?|active:B:?;~P|users|id;~A|users|id|I1;~R|users|I1|TAda|B1;
```

Schema and row records carry explicit tags. Key constraints are metadata records before the row records: `~P|users|id;` declares a primary key, while `~F|posts|user_id|users|id;` declares a foreign key with the default `RESTRICT/RESTRICT` actions. Any nondefault action appends compact delete/update tags: `~F|posts|user_id|users|id|C|R;` is `ON DELETE CASCADE`, `|N|R` is `ON DELETE SET NULL`, and `|R|C` is `ON UPDATE CASCADE`; combined actions use `|C|C` or `|N|C`. An auto-incrementing key has exactly one record such as `~A|users|id|I42;`, placed after that table's primary- and foreign-key metadata. Its nonnegative high-water mark must cover every stored key for the generated column.

V2 remains the canonical format for databases that use only legacy metadata. A first nonredundant V3 feature—DEFAULT, UNIQUE, CHECK, or a nondefault foreign-key action—atomically changes the header to `V3;`. DEFAULT records such as `~D|jobs|state|Tqueued;` use canonical typed cells, with explicit `DEFAULT NULL` encoded as `N`; UNIQUE records use `~U|users|email;`. Each CHECK is a `~C` record containing a resolved, column-index-based flat preorder program, for example `~C|tasks|GE|1|I0;`. Logical nodes store child counts; LIKE stores wildcard/literal atoms; IN stores canonical typed cells. Per table, DEFAULT records follow optional auto-increment metadata in increasing column order, followed by UNIQUE records in increasing column order and CHECK records in declaration order. Loading accepts V2 and V3 without rewriting either one, V3 never downgrades during later mutations, and a V3-only record—including an extended six-field `~F` record—under a V2 header is corruption. Redundant primary-key UNIQUE declarations and default `RESTRICT/RESTRICT` actions retain legacy metadata and do not require V3. V1 blobs remain unsupported rather than being migrated implicitly.

Cell prefixes distinguish text, integers, booleans, and nulls, while structural and line-breaking characters are escaped reversibly. Loading validates the complete header, schemas, constraint metadata, key integrity, escapes, row widths, types, and canonical encoding; malformed records are never silently skipped.

The format is inspectable for fun and debugging, but callers should treat it as an encoded value rather than edit it by hand. Use `varchar dump` or `Database::as_str()` to see it.

