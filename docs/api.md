# Finelor Public API

## Overview

Public API endpoints are mounted under:

```text
/api/v1
```

Every public API request requires:

```text
Authorization: Bearer <api_key>
```

API keys are created and managed in the web client under Settings -> Channels -> API.

Available endpoints:

```text
POST /api/v1/documents
GET  /api/v1/documents
GET  /api/v1/documents/status
GET  /api/v1/documents/{short_ref}
GET  /api/v1/documents/{short_ref}/explain
GET  /api/v1/documents/{short_ref}/file
```

Errors use a JSON envelope unless the endpoint returns a file:

```json
{
  "error": "unauthorized"
}
```

Common statuses:

| Status | Meaning |
| --- | --- |
| `400` | Invalid request body or missing required multipart field. |
| `401` | Missing, invalid, revoked, or removed API key. |
| `403` | File access was denied. |
| `404` | Document or source file was not found. |
| `500` | Server-side processing failed. |

## POST /api/v1/documents

Ingests a document into Finelor.

Request content type:

```text
multipart/form-data
```

Multipart fields:

| Field | Required | Description |
| --- | --- | --- |
| `file` | Yes | Source document bytes. Can be an image, PDF, Word file, or other supported document artifact. |
| `filename` | No | Overrides the uploaded file name used for provenance/display. |
| `mime_type` | No | Overrides the uploaded part content type. Defaults to `application/octet-stream` when unavailable. |
| `source_id` | No | Caller-provided idempotency/provenance identifier for the source system. |

Optional headers:

| Header | Description |
| --- | --- |
| `X-Source-Timestamp` | Source-system timestamp stored as provenance metadata. |
| `X-Submitter-ID` | External submitter/profile identifier stored as provenance metadata. |

Response DTO:

```json
{
  "document_id": 123,
  "short_ref": "D000123",
  "status": "RECEIVED"
}
```

Fields:

| Field | Type | Description |
| --- | --- | --- |
| `document_id` | integer | Internal numeric document id. |
| `short_ref` | string | Stable public document reference used by the public API. |
| `status` | string | Initial document status after ingestion. |

## GET /api/v1/documents

Returns a paginated document list.

Query parameters:

| Parameter | Type | Default | Description |
| --- | --- | --- | --- |
| `limit` | integer | `50` | Page size. Clamped to `1..200`. |
| `offset` | integer | `0` | Zero-based row offset. Negative values are treated as `0`. |
| `status` | string | none | Exact document status filter. |
| `month` | string | none | Received month filter in `YYYY-MM` format. |
| `search` | string | none | Case-insensitive search over `short_ref` and supplier name. |

Response DTO:

```json
{
  "items": [
    {
      "short_ref": "D000123",
      "status": "VALIDATED",
      "supplier_name": "Example AB",
      "transaction_date": "2026-05-25",
      "total_amount": "1250.00",
      "received_date": "2026-05-25",
      "ai_confidence": 0.94,
      "model_used": "model-name",
      "has_download": true,
      "download_url": "/api/v1/documents/D000123/file"
    }
  ],
  "limit": 50,
  "offset": 0,
  "total": 1
}
```

`DocumentListItem` fields:

| Field | Type | Description |
| --- | --- | --- |
| `short_ref` | string | Stable public document reference. |
| `status` | string | Current document pipeline status. |
| `supplier_name` | string or null | Latest extracted supplier name. |
| `transaction_date` | string or null | Latest extracted transaction date. |
| `total_amount` | string or null | Latest extracted total amount. Amounts are serialized as strings to preserve decimal precision. |
| `received_date` | string or null | Date the document was received. |
| `ai_confidence` | number or null | Latest accounting confidence score. |
| `model_used` | string or null | Latest accounting model identifier. |
| `has_download` | boolean | Whether a source artifact is available through the public API. |
| `download_url` | string or null | File download endpoint for the source artifact when `has_download` is `true`. |

Pagination fields:

| Field | Type | Description |
| --- | --- | --- |
| `limit` | integer | Effective page size after clamping. |
| `offset` | integer | Effective offset. |
| `total` | integer | Total matching documents before pagination. |

## GET /api/v1/documents/status

Returns document counts grouped by processing status category.

Response DTO:

```json
{
  "processing": 3,
  "pending_review": 1,
  "export_ready": 2,
  "exported": 10,
  "failed": 0
}
```

Fields:

| Field | Type | Description |
| --- | --- | --- |
| `processing` | integer | Documents currently in the processing pipeline. |
| `pending_review` | integer | Documents waiting for human review. |
| `export_ready` | integer | Documents ready for export. |
| `exported` | integer | Documents already exported. |
| `failed` | integer | Documents in failed state. |

## GET /api/v1/documents/{short_ref}

Returns detailed information for one document.

Response DTO:

```json
{
  "short_ref": "D000123",
  "status": "VALIDATED",
  "filename": "invoice.pdf",
  "mime_type": "application/pdf",
  "supplier_name": "Example AB",
  "transaction_date": "2026-05-25",
  "total_amount": "1250.00",
  "vat_amount": "250.00",
  "subtotal_amount": "1000.00",
  "net_amount": "1000.00",
  "assigned_account_code": "4000",
  "account_name": "Purchases",
  "ai_confidence": 0.94,
  "model_used": "model-name",
  "document_type": "INVOICE",
  "review_confidence": 0.91,
  "review_decision_type": "APPROVED",
  "review_reason": "Validated automatically",
  "validation_errors": null,
  "accounting_rows": [
    {
      "account_code": "4000",
      "description": "Purchase",
      "amount": "1000.00",
      "vat_code": "25",
      "is_debit": true
    }
  ],
  "has_download": true,
  "download_url": "/api/v1/documents/D000123/file"
}
```

Fields:

| Field | Type | Description |
| --- | --- | --- |
| `short_ref` | string | Stable public document reference. |
| `status` | string | Current document pipeline status. |
| `filename` | string or null | Stored/display file name. |
| `mime_type` | string or null | Stored MIME type for the source artifact. |
| `supplier_name` | string or null | Latest extracted supplier name. |
| `transaction_date` | string or null | Latest extracted transaction date. |
| `total_amount` | string or null | Latest extracted total amount. |
| `vat_amount` | string or null | Latest extracted VAT amount. |
| `subtotal_amount` | string or null | Latest extracted subtotal amount. |
| `net_amount` | string or null | Latest accounting net amount. |
| `assigned_account_code` | string or null | Latest assigned account code. |
| `account_name` | string or null | Latest assigned account name. |
| `ai_confidence` | number or null | Latest accounting confidence score. |
| `model_used` | string or null | Latest accounting model identifier. |
| `document_type` | string or null | Latest extracted document type. |
| `review_confidence` | number or null | Latest review confidence score. |
| `review_decision_type` | string or null | Latest review decision type. |
| `review_reason` | string or null | Latest review reason. |
| `validation_errors` | object/array/string or null | Latest validation error payload, when present. |
| `accounting_rows` | array | Latest account assignment rows for the document. |
| `has_download` | boolean | Whether a source artifact is available through the public API. |
| `download_url` | string or null | File download endpoint for the source artifact when `has_download` is `true`. |

`AccountingRow` fields:

| Field | Type | Description |
| --- | --- | --- |
| `account_code` | string | Assigned account code. |
| `description` | string or null | Assignment description. |
| `amount` | string or null | Assignment amount serialized as a string to preserve decimal precision. |
| `vat_code` | string or null | VAT code when assigned. |
| `is_debit` | boolean or null | Whether the row is a debit entry. |

## GET /api/v1/documents/{short_ref}/explain

Returns a human-readable explanation of the current document state.

Response DTO:

```json
{
  "short_ref": "D000123",
  "explanation": "D000123 is currently EXPORT_READY.\nReason: this document is healthy and ready for export.\nReview: AUTO_APPROVED\nConfidence: 91%"
}
```

Fields:

| Field | Type | Description |
| --- | --- | --- |
| `short_ref` | string | Stable public document reference. |
| `explanation` | string | Human-readable explanation of the current state and relevant review details. |

## GET /api/v1/documents/{short_ref}/file

Downloads the stored source artifact for a document.

Response:

| Header | Description |
| --- | --- |
| `Content-Type` | Stored document MIME type, or `application/octet-stream` if unknown. |
| `Content-Disposition` | `attachment`, with `filename="<filename>"` when available. |

The response body is the raw file bytes. Use the `download_url` from the list or detail DTO when available.
