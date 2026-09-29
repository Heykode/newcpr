CREATE INDEX request_capture_records_request
    ON request_capture_records ((record->>'requestId'), created_at DESC);
