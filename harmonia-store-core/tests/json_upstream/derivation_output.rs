//! DerivationOutput JSON tests

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::derivation::DerivationOutput;
use harmonia_store_core::store_path::{ContentAddress, ContentAddressMethodAlgorithm};
use harmonia_utils_hash::{Algorithm, Hash};
use hex_literal::hex;

test_upstream_json!(
    test_derivation_output_input_addressed,
    libstore_test_data_path("derivation/output-inputAddressed.json"),
    {
        DerivationOutput::InputAddressed(
            "c015dhfh5l0lp6wxyvdn7bmwhbbr6hr9-drv-name-output-name"
                .parse()
                .unwrap(),
        )
    }
);

test_upstream_json!(
    test_derivation_output_ca_fixed_flat,
    libstore_test_data_path("derivation/output-caFixedFlat.json"),
    {
        DerivationOutput::CAFixed(ContentAddress::Flat(Hash::new(
            Algorithm::SHA256,
            &hex!("894517c9163c896ec31a2adbd33c0681fd5f45b2c0ef08a64c92a03fb97f390f"),
        )))
    }
);

test_upstream_json!(
    test_derivation_output_ca_fixed_nar,
    libstore_test_data_path("derivation/output-caFixedNAR.json"),
    {
        DerivationOutput::CAFixed(ContentAddress::Recursive(Hash::new(
            Algorithm::SHA256,
            &hex!("894517c9163c896ec31a2adbd33c0681fd5f45b2c0ef08a64c92a03fb97f390f"),
        )))
    }
);

test_upstream_json!(
    test_derivation_output_ca_fixed_text,
    libstore_test_data_path("derivation/output-caFixedText.json"),
    {
        DerivationOutput::CAFixed(ContentAddress::Text(
            Hash::new(
                Algorithm::SHA256,
                &hex!("894517c9163c896ec31a2adbd33c0681fd5f45b2c0ef08a64c92a03fb97f390f"),
            )
            .try_into()
            .unwrap(),
        ))
    }
);

test_upstream_json!(
    test_derivation_output_ca_floating,
    libstore_test_data_path("derivation/output-caFloating.json"),
    { DerivationOutput::CAFloating(ContentAddressMethodAlgorithm::Recursive(Algorithm::SHA256)) }
);

test_upstream_json!(
    test_derivation_output_deferred,
    libstore_test_data_path("derivation/output-deferred.json"),
    { DerivationOutput::Deferred }
);

test_upstream_json!(
    test_derivation_output_impure,
    libstore_test_data_path("derivation/output-impure.json"),
    { DerivationOutput::Impure(ContentAddressMethodAlgorithm::Recursive(Algorithm::SHA256)) }
);
