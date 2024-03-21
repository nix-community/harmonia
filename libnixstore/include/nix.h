#pragma once

#include "rust/cxx.h"
#include "libnixstore/src/lib.rs.h"

namespace libnixstore {
void init();
void set_verbosity(int32_t level);
bool is_valid_path(rust::Str path);
rust::Vec<rust::String> query_references(rust::Str path);
rust::String query_path_hash(rust::Str path);
rust::String query_deriver(rust::Str path);
InternalPathInfo query_path_info(rust::Str path, bool base32);
rust::String query_raw_realisation(rust::Str output_id);
rust::String query_path_from_hash_part(rust::Str hash_part);
rust::Vec<rust::String> compute_fs_closure(bool flip_direction,
                                           bool include_outputs,
                                           rust::Vec<rust::Str> paths);
rust::Vec<rust::String> topo_sort_paths(rust::Vec<rust::Str> paths);
rust::String follow_links_to_store_path(rust::Str path);
void export_paths(int32_t fd, rust::Vec<rust::Str> paths);
void import_paths(int32_t fd, bool dont_check_signs);
rust::String hash_file(rust::Str algo, bool base32, rust::Str path);
rust::String hash_string(rust::Str algo, bool base32, rust::Str s);
rust::String convert_hash(rust::Str algo, rust::Str s, bool to_base_32);
rust::String sign_string(rust::Str secret_key, rust::Str msg);
bool check_signature(rust::Str public_key, rust::Str sig, rust::Str msg);
rust::String add_to_store(rust::Str src_path, int32_t recursive,
                          rust::Str algo);
InternalDrv derivation_from_path(rust::Str drv_path);
void add_temp_root(rust::Str store_path);
rust::String get_bin_dir();
rust::String get_store_dir();

rust::String get_build_log(rust::Str derivation_path);
rust::String get_nar_list(rust::Str store_path);

} // namespace libnixstore
