table! {
    auth (id) {
        id -> Text,
        phc -> Text,
    }
}

table! {
    file (id) {
        id -> Integer,
        token_id -> Integer,
        name -> Nullable<Text>,
        path -> Text,
        content_type -> Nullable<Text>,
        size_mib -> Nullable<Integer>,
        created_at -> Timestamp,
        deleted_at -> Nullable<Timestamp>,
        file_upload_status -> Text,
    }
}

table! {
    token (id) {
        id -> Integer,
        path -> Text,
        status -> Text,
        max_size_mib -> Nullable<Integer>,
        created_at -> Timestamp,
        token_expires_at -> Timestamp,
        content_expires_at -> Nullable<Timestamp>,
        content_expires_after_hours -> Nullable<Integer>,
        deleted_at -> Nullable<Timestamp>,
    }
}

joinable!(file -> token (token_id));

allow_tables_to_appear_in_same_query!(
    auth,
    file,
    token,
);
