#pragma once

#include "rust/cxx.h"
#include "libnixstore/src/lib.rs.h"

namespace libnixstore {
void init();
bool is_valid_path(rust::Str path);
rust::String query_path_hash(rust::Str path);
InternalPathInfo query_path_info(rust::Str path, bool base32);
rust::String query_path_from_hash_part(rust::Str hash_part);
rust::String convert_hash(rust::Str algo, rust::Str s, bool to_base_32);
rust::String sign_string(rust::Str secret_key, rust::Str msg);
bool check_signature(rust::Str public_key, rust::Str sig, rust::Str msg);
InternalDrv derivation_from_path(rust::Str drv_path);
rust::String get_store_dir();
rust::String get_build_log(rust::Str derivation_path);
rust::String get_nar_list(rust::Str store_path);

} // namespace libnixstore
