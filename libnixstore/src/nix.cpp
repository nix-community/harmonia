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
#include <sodium.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

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
    _store = openStore(nix::settings.storeUri, params);
  }
  return nix::ref<nix::Store>(_store);
}

static nix::DerivedPath to_derived_path(const nix::StorePath &store_path) {
  if (store_path.isDerivation()) {
    auto drv = get_store()->readDerivation(store_path);
    return nix::DerivedPath::Built{
        .drvPath = nix::makeConstantStorePathRef(store_path),
        .outputs = drv.outputNames(),
    };
  } else {
    return nix::DerivedPath::Opaque{
        .path = store_path,
    };
  }
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

rust::String convert_hash(rust::Str algo, rust::Str s, bool to_base_32) {
  nix::Hash h = nix::Hash::parseAny(STRING_VIEW(s),
                                    nix::parseHashAlgo(STRING_VIEW(algo)));
  return h.to_string(
      to_base_32 ? nix::HashFormat::Nix32 : nix::HashFormat::Base16, false);
}

rust::String sign_string(rust::Str secret_key, rust::Str msg) {
  return nix::SecretKey(STRING_VIEW(secret_key)).signDetached(STRING_VIEW(msg));
}

bool check_signature(rust::Str public_key, rust::Str sig, rust::Str msg) {
  if (public_key.length() != crypto_sign_PUBLICKEYBYTES) {
    throw nix::Error("public key is not valid");
  }
  if (sig.length() != crypto_sign_BYTES) {
    throw nix::Error("signature is not valid");
  }
  return crypto_sign_verify_detached((unsigned char *)sig.data(),
                                     (unsigned char *)msg.data(), msg.length(),
                                     (unsigned char *)public_key.data()) == 0;
}

InternalDrv derivation_from_path(rust::Str drv_path) {
  auto store = get_store();
  nix::Derivation drv =
      store->derivationFromPath(store->parseStorePath(STRING_VIEW(drv_path)));

  auto oaop = drv.outputsAndOptPaths(*store);
  rust::Vec<InternalTuple> outputs;
  outputs.reserve(oaop.size());
  for (auto &i : oaop) {
    outputs.push_back(InternalTuple{
        i.first,
        i.second.second ? store->printStorePath(*i.second.second) : ""});
  }

  rust::Vec<rust::String> input_drvs;
  input_drvs.reserve(drv.inputDrvs.map.size());
  for (const auto &[inputDrv, inputNode] : drv.inputDrvs.map) {
    input_drvs.push_back(store->printStorePath(inputDrv));
  }

  rust::Vec<rust::String> input_srcs = extract_path_set(drv.inputSrcs);

  rust::Vec<rust::String> args;
  args.reserve(drv.args.size());
  for (const std::string &i : drv.args) {
    args.push_back(i);
  }

  rust::Vec<InternalTuple> env;
  env.reserve(drv.env.size());
  for (auto &i : drv.env) {
    env.push_back(InternalTuple{i.first, i.second});
  }

  return InternalDrv{
      outputs, input_drvs, input_srcs, drv.platform, drv.builder, args, env,
  };
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

rust::String get_build_log(rust::Str derivation_path) {
  auto store = get_store();
  auto path = store->parseStorePath(STRING_VIEW(derivation_path));
  auto subs = nix::getDefaultSubstituters();

  subs.push_front(store);
  auto b = to_derived_path(path);

  for (auto &sub : subs) {
    nix::LogStore *log_store = dynamic_cast<nix::LogStore *>(&*sub);
    if (!log_store) {
      continue;
    }
    std::optional<std::string> log = std::visit(
        overloaded{
            [&](const nix::DerivedPath::Opaque &bo) {
              return log_store->getBuildLog(bo.path);
            },
            [&](const nix::DerivedPath::Built &bfd) {
              return log_store->getBuildLog(bfd.drvPath->getBaseStorePath());
            },
        },
        b.raw());
    if (!log) {
      continue;
    }
    return *log;
  }
  // TODO(conni2461): Replace with option
  return "";
}

rust::String get_nar_list(rust::Str store_path) {
  auto path = nix::CanonPath(STRING_VIEW(store_path));
  nlohmann::json j = {
      {"version", 1},
      {"root", listNar(get_store()->getFSAccessor(), path, true)},
  };

  return j.dump();
}

class StopDump : public std::exception {
public:
  const char *what() const noexcept override {
    return "Stop dumping nar";
  }
};
} // namespace libnixstore
