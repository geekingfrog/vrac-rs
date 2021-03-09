table! {
    token (id) {
        id -> Integer,
        path -> Text,
        status -> Text,
        max_size -> Nullable<Integer>,
        created_at -> Timestamp,
        token_expires_at -> Timestamp,
        content_expires_at -> Nullable<Timestamp>,
        deleted_at -> Nullable<Timestamp>,
    }
}
