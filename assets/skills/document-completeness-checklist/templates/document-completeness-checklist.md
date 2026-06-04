Use this shape when responding with a document completeness checklist.

Limitation note:
`This checklist is based on the recent documents currently accessible through tools and may not cover the full historical workspace.`

Example layout:

## D000123
Status: `PENDING_HUMAN_REVIEW`

- Supplier name: complete
- Invoice date: complete
- Total amount: missing
- Missing items: total amount

## D000122
Status: `EXPORT_READY`

- Supplier name: complete
- Invoice date: missing
- Total amount: complete
- Missing items: invoice date

If a document has no missing checklist fields, say:

- `Missing items: none visible in the current tool output`

If no recent documents are returned, say:

`No recent documents were returned by the current tool surface, so there is nothing to checklist right now.`
