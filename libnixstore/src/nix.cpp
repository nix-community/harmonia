#include "libnixstore/include/nix.h"

#include <nix/config.h>
#include <nix/derivations.hh>
#include <nix/globals.hh>
#include <nix/shared.hh>
#include <nix/store-api.hh>
#include <nix/log-store.hh>
#include <nix/content-address.hh>
#include <nix/util.hh>
#include <nix/crypto.hh>

#include <nix/nar-accessor.hh>

#include <nlohmann/json.hpp>
#include <sodium.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

// C++17 std::visit boilerplate
template <class... Ts> struct overloaded : Ts... { using Ts::operator()...; };
template <class... Ts> overloaded(Ts...) -> overloaded<Ts...>;

static nix::ref<nix::Store> get_store() {
  static std::shared_ptr<nix::Store> _store;
  if (!_store) {
    nix::initLibStore();

    nix::loadConfFile();
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

void set_verbosity(int32_t level) {
  nix::verbosity = (nix::Verbosity)level;
}

bool is_valid_path(rust::Str path) {
  auto store = get_store();
  return store->isValidPath(store->parseStorePath(STRING_VIEW(path)));
}

rust::Vec<rust::String> nix_query_references(rust::Str path) {
  auto store = get_store();
  return extract_path_set(
      store->queryPathInfo(store->parseStorePath(STRING_VIEW(path)))
          ->references);
}

rust::String query_path_hash(rust::Str path) {
  auto store = get_store();
  return store->queryPathInfo(store->parseStorePath(STRING_VIEW(path)))
      ->narHash.to_string(nix::Base32, true);
}

rust::String query_deriver(rust::Str path) {
  auto store = get_store();
  nix::ref<const nix::ValidPathInfo> info =
      store->queryPathInfo(store->parseStorePath(STRING_VIEW(path)));
  return extract_opt_path(info->deriver);
}

InternalPathInfo query_path_info(rust::Str path, bool base32) {
  auto store = get_store();
  nix::ref<const nix::ValidPathInfo> info =
      store->queryPathInfo(store->parseStorePath(STRING_VIEW(path)));

  std::string narhash =
      info->narHash.to_string(base32 ? nix::Base32 : nix::Base16, true);

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

rust::String query_raw_realisation(rust::Str output_id) {
  std::shared_ptr<const nix::Realisation> realisation =
      get_store()->queryRealisation(
          nix::DrvOutput::parse(STRING_VIEW(output_id)));
  if (!realisation) {
    // TODO(conni2461): Replace with option
    return "";
  }
  return realisation->toJSON().dump();
}

rust::String query_path_from_hash_part(rust::Str hash_part) {
  return extract_opt_path(
      get_store()->queryPathFromHashPart(STRING_VIEW(hash_part)));
}

rust::Vec<rust::String> compute_fs_closure(bool flip_direction,
                                           bool include_outputs,
                                           rust::Vec<rust::Str> paths) {
  auto store = get_store();
  nix::StorePathSet path_set;
  for (auto &path : paths) {
    store->computeFSClosure(store->parseStorePath(STRING_VIEW(path)), path_set,
                            flip_direction, include_outputs);
  }
  return extract_path_set(path_set);
}

rust::Vec<rust::String> topo_sort_paths(rust::Vec<rust::Str> paths) {
  auto store = get_store();
  nix::StorePathSet path_set;
  for (auto &path : paths) {
    path_set.insert(store->parseStorePath(STRING_VIEW(path)));
  }
  nix::StorePaths sorted = store->topoSortPaths(path_set);
  rust::Vec<rust::String> res;
  res.reserve(sorted.size());
  for (auto &path : sorted) {
    res.push_back(store->printStorePath(path));
  }
  return res;
}

rust::String follow_links_to_store_path(rust::Str path) {
  auto store = get_store();
  return store->printStorePath(
      store->followLinksToStorePath(STRING_VIEW(path)));
}

void export_paths(int32_t fd, rust::Vec<rust::Str> paths) {
  auto store = get_store();
  nix::StorePathSet path_set;
  for (auto &path : paths) {
    path_set.insert(store->parseStorePath(STRING_VIEW(path)));
  }
  nix::FdSink sink(fd);
  store->exportPaths(path_set, sink);
}

void import_paths(int32_t fd, bool dont_check_signs) {
  nix::FdSource source(fd);
  get_store()->importPaths(source, dont_check_signs ? nix::NoCheckSigs
                                                    : nix::CheckSigs);
}

rust::String hash_path(rust::Str algo, bool base32, rust::Str path) {
  nix::Hash h =
      nix::hashPath(nix::parseHashType(STRING_VIEW(algo)), STRING_VIEW(path))
          .first;
  return h.to_string(base32 ? nix::Base32 : nix::Base16, false);
}

rust::String hash_file(rust::Str algo, bool base32, rust::Str path) {
  nix::Hash h =
      nix::hashFile(nix::parseHashType(STRING_VIEW(algo)), STRING_VIEW(path));
  return h.to_string(base32 ? nix::Base32 : nix::Base16, false);
}

rust::String hash_string(rust::Str algo, bool base32, rust::Str s) {
  nix::Hash h =
      nix::hashString(nix::parseHashType(STRING_VIEW(algo)), STRING_VIEW(s));
  return h.to_string(base32 ? nix::Base32 : nix::Base16, false);
}

rust::String convert_hash(rust::Str algo, rust::Str s, bool to_base_32) {
  nix::Hash h = nix::Hash::parseAny(STRING_VIEW(s),
                                    nix::parseHashType(STRING_VIEW(algo)));
  return h.to_string(to_base_32 ? nix::Base32 : nix::Base16, false);
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

rust::String add_to_store(rust::Str src_path, int32_t recursive,
                          rust::Str algo) {
  auto store = get_store();
  nix::FileIngestionMethod method = recursive
                                        ? nix::FileIngestionMethod::Recursive
                                        : nix::FileIngestionMethod::Flat;
  nix::StorePath path = store->addToStore(
      std::string(nix::baseNameOf(STRING_VIEW(src_path))),
      STRING_VIEW(src_path), method, nix::parseHashType(STRING_VIEW(algo)));
  return store->printStorePath(path);
}

rust::String make_fixed_output_path(bool recursive, rust::Str algo,
                                    rust::Str hash, rust::Str name) {
  auto store = get_store();

  nix::FixedOutputInfo info{
      .method = recursive ? nix::FileIngestionMethod::Recursive
                          : nix::FileIngestionMethod::Flat,
      .hash = nix::Hash::parseAny(STRING_VIEW(hash),
                                  nix::parseHashType(STRING_VIEW(algo))),
      .references = {},
  };

  nix::StorePath path = store->makeFixedOutputPath(STRING_VIEW(name), info);
  return store->printStorePath(path);
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

void add_temp_root(rust::Str store_path) {
  auto store = get_store();
  store->addTempRoot(store->parseStorePath(STRING_VIEW(store_path)));
}

rust::String get_bin_dir() {
  return nix::settings.nixBinDir;
}

rust::String get_store_dir() {
  return nix::settings.nixStore;
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
  nlohmann::json j = {
      {"version", 1},
      {"root",
       listNar(get_store()->getFSAccessor(), STRING_VIEW(store_path), true)},
  };

  return j.dump();
}

class StopDump : public std::exception {
public:
  const char *what() {
    return "Stop dumping nar";
  }
};
} // namespace libnixstore
