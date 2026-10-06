//! One vote per real person.
//!
//! `player_link` is 1:1 (one Minecraft account per Discord account), so the
//! remaining way to vote twice is several Discord accounts, each linked to a
//! different Minecraft alt. Accounts that shared a keyed IP hash in the days
//! before the cut-off are treated as one person. A hash seen on more than
//! `hub_limit` accounts (VPN, carrier or cafe address) links nobody.

use std::collections::{HashMap, HashSet};

use crate::database::normalize_uuid;

/// Person clusters over Minecraft accounts (normalized uuids).
#[derive(Clone, Debug, Default)]
pub struct Identity {
    parent: HashMap<String, String>,
    smallest: HashMap<String, String>,
    /// Non-crowded IP hashes per account, for the direct "shared an address" test.
    hashes: HashMap<String, HashSet<Vec<u8>>>,
}

impl Identity {
    /// Builds clusters from `(uuid, ip_hash)` observations.
    pub fn build(observations: &[(String, Vec<u8>)], hub_limit: usize) -> Self {
        let mut accounts_by_hash: HashMap<&[u8], HashSet<String>> = HashMap::new();
        for (uuid, hash) in observations {
            accounts_by_hash
                .entry(hash.as_slice())
                .or_default()
                .insert(normalize_uuid(uuid));
        }
        let mut identity = Self::default();
        for (hash, accounts) in &accounts_by_hash {
            if accounts.len() > hub_limit {
                continue;
            }
            for account in accounts {
                identity
                    .hashes
                    .entry(account.clone())
                    .or_default()
                    .insert(hash.to_vec());
            }
            let mut iter = accounts.iter();
            let Some(first) = iter.next() else {
                continue;
            };
            for other in iter {
                identity.union(first, other);
            }
        }
        identity.finish();
        identity
    }

    fn find(&mut self, uuid: &str) -> String {
        let mut root = uuid.to_owned();
        while let Some(parent) = self.parent.get(&root) {
            if *parent == root {
                break;
            }
            root.clone_from(parent);
        }
        // Path compression.
        let mut current = uuid.to_owned();
        while current != root {
            let next = self
                .parent
                .insert(current.clone(), root.clone())
                .unwrap_or_else(|| root.clone());
            current = next;
        }
        self.parent
            .entry(root.clone())
            .or_insert_with(|| root.clone());
        root
    }

    fn union(&mut self, a: &str, b: &str) {
        let root_a = self.find(a);
        let root_b = self.find(b);
        if root_a != root_b {
            self.parent.insert(root_b, root_a);
        }
    }

    /// Resolves every cluster's smallest uuid, the stable person key.
    fn finish(&mut self) {
        let members: Vec<String> = self.parent.keys().cloned().collect();
        let mut smallest: HashMap<String, String> = HashMap::new();
        for member in members {
            let root = self.find(&member);
            smallest
                .entry(root)
                .and_modify(|current| {
                    if member < *current {
                        current.clone_from(&member);
                    }
                })
                .or_insert(member);
        }
        self.smallest = smallest;
    }

    /// The person key of an account: the smallest uuid in its cluster, or the
    /// account itself when it shares no usable IP hash with anybody.
    pub fn person_key(&self, uuid: &str) -> String {
        let uuid = normalize_uuid(uuid);
        let mut root = uuid.clone();
        while let Some(parent) = self.parent.get(&root) {
            if *parent == root {
                break;
            }
            root.clone_from(parent);
        }
        self.smallest.get(&root).cloned().unwrap_or(uuid)
    }

    /// Whether two accounts joined from the same non-crowded address in the
    /// window (a direct link, not a chain through a third account).
    pub fn shares_ip(&self, a: &str, b: &str) -> bool {
        match (
            self.hashes.get(&normalize_uuid(a)),
            self.hashes.get(&normalize_uuid(b)),
        ) {
            (Some(first), Some(second)) => !first.is_disjoint(second),
            _ => false,
        }
    }

    #[cfg(test)]
    pub fn same_person(&self, a: &str, b: &str) -> bool {
        self.person_key(a) == self.person_key(b)
    }
}

/// A linked account that passed the poll's class expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub discord_id: String,
    pub uuid: String,
    /// When the link was made, epoch seconds. `i64::MAX` when unknown.
    pub linked_at: i64,
}

/// One snapshot row: the Discord account allowed to vote for a person.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Voter {
    pub discord_id: String,
    pub uuid: String,
    pub person_key: String,
}

/// Keeps one Discord account per person: the one linked first (then the lowest
/// id). The other accounts of the same person are simply not in the snapshot.
pub fn pick_voters(candidates: Vec<Candidate>, identity: &Identity) -> Vec<Voter> {
    let mut best: HashMap<String, Candidate> = HashMap::new();
    for candidate in candidates {
        let key = identity.person_key(&candidate.uuid);
        match best.get(&key) {
            Some(current) if !is_earlier(&candidate, current) => {}
            _ => {
                best.insert(key, candidate);
            }
        }
    }
    let mut voters: Vec<Voter> = best
        .into_iter()
        .map(|(person_key, candidate)| Voter {
            discord_id: candidate.discord_id,
            uuid: normalize_uuid(&candidate.uuid),
            person_key,
        })
        .collect();
    voters.sort_by(|a, b| a.person_key.cmp(&b.person_key));
    voters
}

fn is_earlier(candidate: &Candidate, current: &Candidate) -> bool {
    (candidate.linked_at, discord_order(&candidate.discord_id))
        < (current.linked_at, discord_order(&current.discord_id))
}

fn discord_order(id: &str) -> (usize, &str) {
    // Snowflakes compare numerically: shorter first, then lexicographically.
    (id.len(), id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(uuid: &str, hash: u8) -> (String, Vec<u8>) {
        (uuid.to_owned(), vec![hash; 12])
    }

    #[test]
    fn accounts_sharing_an_ip_hash_are_one_person() {
        let identity = Identity::build(
            &[
                obs("aa", 1),
                obs("bb", 1),
                obs("cc", 2),
                obs("bb", 3),
                obs("dd", 3),
            ],
            4,
        );
        assert!(identity.same_person("aa", "bb"));
        assert!(identity.same_person("aa", "dd"), "linked through bb");
        assert!(!identity.same_person("aa", "cc"));
        assert_eq!(identity.person_key("dd"), "aa");
        assert_eq!(identity.person_key("cc"), "cc");
        assert_eq!(identity.person_key("neverseen"), "neverseen");
    }

    #[test]
    fn shares_ip_is_direct_only() {
        let identity = Identity::build(&[obs("a", 1), obs("b", 1), obs("b", 2), obs("c", 2)], 4);
        assert!(identity.shares_ip("a", "b"));
        assert!(identity.shares_ip("b", "c"));
        assert!(
            !identity.shares_ip("a", "c"),
            "a and c are linked only through b"
        );
        assert!(
            identity.same_person("a", "c"),
            "but they are one voting cluster"
        );
        let crowd: Vec<_> = ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(|u| obs(u, 9))
            .collect();
        assert!(!Identity::build(&crowd, 4).shares_ip("a", "b"));
    }

    #[test]
    fn dashes_and_case_do_not_matter() {
        let identity = Identity::build(
            &[
                obs("AABBCCDD-0000-0000-0000-000000000001", 1),
                obs("aabbccdd000000000000000000000002", 1),
            ],
            4,
        );
        assert!(identity.same_person(
            "aabbccdd-0000-0000-0000-000000000001",
            "AABBCCDD-0000-0000-0000-000000000002"
        ));
    }

    #[test]
    fn hub_addresses_link_nobody() {
        let crowd: Vec<_> = ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(|uuid| obs(uuid, 9))
            .collect();
        let identity = Identity::build(&crowd, 4);
        assert!(!identity.same_person("a", "b"));
        // Exactly at the limit still links.
        let at_limit = Identity::build(&crowd[..4], 4);
        assert!(at_limit.same_person("a", "d"));
    }

    #[test]
    fn a_hub_does_not_break_a_real_link_elsewhere() {
        let mut observations: Vec<_> = (0..6).map(|i| obs(&format!("x{i}"), 7)).collect();
        observations.push(obs("x0", 8));
        observations.push(obs("x1", 8));
        let identity = Identity::build(&observations, 4);
        assert!(identity.same_person("x0", "x1"));
        assert!(!identity.same_person("x0", "x2"));
    }

    fn candidate(discord: &str, uuid: &str, linked_at: i64) -> Candidate {
        Candidate {
            discord_id: discord.into(),
            uuid: uuid.into(),
            linked_at,
        }
    }

    #[test]
    fn one_discord_account_per_person_the_earliest_link() {
        let identity = Identity::build(&[obs("u1", 1), obs("u2", 1), obs("u3", 2)], 4);
        let voters = pick_voters(
            vec![
                candidate("200", "u2", 50),
                candidate("100", "u1", 80),
                candidate("300", "u3", 10),
            ],
            &identity,
        );
        assert_eq!(voters.len(), 2);
        let ids: Vec<_> = voters
            .iter()
            .map(|voter| voter.discord_id.as_str())
            .collect();
        assert!(ids.contains(&"200"), "linked first within the cluster");
        assert!(!ids.contains(&"100"));
        assert!(ids.contains(&"300"));
        // Person keys are unique, which the poll_eligible table also enforces.
        let keys: HashSet<_> = voters
            .iter()
            .map(|voter| voter.person_key.clone())
            .collect();
        assert_eq!(keys.len(), voters.len());
    }

    #[test]
    fn ties_break_on_the_lower_discord_id() {
        let identity = Identity::build(&[obs("u1", 1), obs("u2", 1)], 4);
        let voters = pick_voters(
            vec![candidate("1000", "u1", 5), candidate("999", "u2", 5)],
            &identity,
        );
        assert_eq!(voters.len(), 1);
        assert_eq!(voters[0].discord_id, "999");
    }

    #[test]
    fn unrelated_accounts_all_stay() {
        let identity = Identity::build(&[], 4);
        let voters = pick_voters(
            vec![candidate("1", "a", 1), candidate("2", "b", 2)],
            &identity,
        );
        assert_eq!(voters.len(), 2);
    }
}
