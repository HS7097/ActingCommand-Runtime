// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S1a (register §1.287, "Slice LAB-6S1a"; contracts/lab-two-entries.md §4.1):
// the six initiator words are written once, in the contract crate, and what this crate writes (the
// recordings' and the installation journal's `initiator`: its type's `as_str` and serialization)
// is that list, in the contract's order, and reads back as the same initiator.

use crate::initiator::Initiator;

#[test]
fn lab_ops_initiator_words_are_the_contracts() {
    let contract = actingcommand_contract::Initiator::ALL;
    assert_eq!(Initiator::ALL.map(Initiator::word), contract);
    for (initiator, word) in Initiator::ALL.into_iter().zip(contract) {
        assert_eq!(initiator.as_str(), word.as_str());
        let written = serde_json::to_value(initiator).expect("the word encodes");
        assert_eq!(
            written,
            serde_json::to_value(word).expect("the contract's word encodes")
        );
        assert_eq!(
            serde_json::from_value::<Initiator>(written).ok(),
            Some(initiator)
        );
    }
}
