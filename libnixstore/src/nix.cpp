#include "libnixstore/include/nix.h"

#include <nix/canon-path.hh>
#include <nix/config.h>
#include <nix/derivations.hh>
#include <nix/globals.hh>
#include <nix/shared.hh>
#include <nix/store-api.hh>
#include <nix/local-fs-store.hh>
#include <nix/log-store.hh>
#include <nix/content-address.hh>
#include <nix/util.hh>

#include <nix/nar-accessor.hh>

#include <nlohmann/json.hpp>

#include <stdlib.h>

// C++17 std::visit boilerplate
template <class... Ts> struct overloaded : Ts... {
  using Ts::operator()...;
};
template <class... Ts> overloaded(Ts...) -> overloaded<Ts...>;

static nix::ref<nix::Store> get_store() {
  static std::shared_ptr<nix::Store> _store;
  if (!_store) {
    nix::initLibStore(true);

    nix::Store::Params params;
    // Disable caching since we run as a deamon and non-reproduceable builds
    // might have a different result for hashes
    params["path-info-cache-size"] = "0";

    // Disable substituting since we don't want to pull from other caches,
    // this also avoids potential recursion
    params["substitute"] = "false";
    _store = openStore(nix::settings.storeUri, params);
  }
  return nix::ref<nix::Store>(_store);
}

static inline rust::String
extract_opt_path(const std::optional<nix::StorePath> &v) {
  // TODO(conni2461): Replace with option
  return v ? get_store()->printStorePath(*v) : "";
}

static inline rust::Vec<rust::String>
extract_path_set(const nix::StorePathSet &set) {
  auto store = get_store();

  rust::Vec<rust::String> data;
  data.reserve(set.size());
  for (const nix::StorePath &path : set) {
    data.push_back(store->printStorePath(path));
  }
  return data;
}

// shorthand to create std::string_view from rust::Str, we dont wan't to create
// std::string because that involves allocating memory
#define STRING_VIEW(rstr) std::string(rstr.data(), rstr.length())

namespace libnixstore {
void init() {
  get_store();
}

bool is_valid_path(rust::Str path) {
  auto store = get_store();
  return store->isValidPath(store->parseStorePath(STRING_VIEW(path)));
}

rust::String query_path_hash(rust::Str path) {
  auto store = get_store();
  return store->queryPathInfo(store->parseStorePath(STRING_VIEW(path)))
      ->narHash.to_string(nix::HashFormat::Nix32, true);
}

InternalPathInfo query_path_info(rust::Str path, bool base32) {
  auto store = get_store();
  nix::ref<const nix::ValidPathInfo> info =
      store->queryPathInfo(store->parseStorePath(STRING_VIEW(path)));

  std::string narhash = info->narHash.to_string(
      base32 ? nix::HashFormat::Nix32 : nix::HashFormat::Base16, true);

  rust::Vec<rust::String> refs = extract_path_set(info->references);

  rust::Vec<rust::String> sigs;
  sigs.reserve(info->sigs.size());
  for (const std::string &sig : info->sigs) {
    sigs.push_back(sig);
  }

  // TODO(conni2461): Replace "" with option
  return InternalPathInfo{
      extract_opt_path(info->deriver),
      narhash,
      info->registrationTime,
      info->narSize,
      refs,
      sigs,
      info->ca ? nix::renderContentAddress(*info->ca) : "",
  };
}

rust::String query_path_from_hash_part(rust::Str hash_part) {
  return extract_opt_path(
      get_store()->queryPathFromHashPart(STRING_VIEW(hash_part)));
}

rust::String get_store_dir() {
  return nix::settings.nixStore;
}

rust::String get_real_store_dir() {
  auto store = get_store();
  auto *fsstore = dynamic_cast<nix::LocalFSStore *>(&(*store));

  if (fsstore != nullptr)
    return fsstore->getRealStoreDir();
  else
    return get_store_dir();
}

} // namespace libnixstore
