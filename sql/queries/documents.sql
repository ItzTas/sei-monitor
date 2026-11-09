-- name: ListDocumentIDs :many
SELECT document_id FROM documents WHERE account_id = @account_id AND procedure_id = @procedure_id;

-- name: ListDocuments :many
SELECT * FROM documents
WHERE account_id = @account_id AND procedure_id = @procedure_id
ORDER BY document_id;

-- name: InsertDocument :exec
INSERT INTO documents (account_id, document_id, procedure_id, formatted_protocol, document_type, name, first_seen_at)
VALUES (@account_id, @document_id, @procedure_id, @formatted_protocol, @document_type, @name, @first_seen_at)
ON CONFLICT (account_id, document_id) DO NOTHING;
