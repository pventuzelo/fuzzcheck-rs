//! The [Pool] is responsible for storing and updating inputs along with
//! their associated code coverage.
//!
//! It assigns a score for each input based on how unique its associated code
//! coverage is. And it can randomly select an input with a probability that
//! is proportional to its score relative to all the other ones.
//!
//! # [Feature]: a unit of code coverage
//!
//! The code coverage of an input is a set of [Feature]. A [Feature] is a value
//! that identifies some behavior of the code that was run. For example, it
//! could say “This edge was reached this many times” or “This comparison
//! instruction was called with these arguments”. In practice, features are not
//! perfectly precise. They won't count the exact number of times a code edge
//! was reached, or record the exact arguments passed to an instruction.
//! This is purely due to performance reasons. The end consequence is that the
//! fuzzer may think that an input is less interesting than it really is.
//!
//! # Policy for adding and removing inputs from the pool
//!
//! The pool will strive to keep as few inputs as possible, and will
//! prioritize small high-scoring inputs over large low-scoring ones. It does
//! so in a couple ways.
//!
//! First, an input will only be added if:
//!
//! 1. It contains a new feature, not seen by any other input in the pool; or
//! 2. It is the smallest input that contains a particular Feature; or
//! 3. It has the same size as the smallest input containing a particular
//! Feature, but it is estimated that it will be higher-scoring than that
//! previous smallest input.
//!
//! Second, following a pool update, any input in the pool that does not meet
//! the above conditions anymore will be removed from the pool.
//!
//! # Scoring of an input
//!
//! The score of an input is computed to be as fair as possible. This
//! is currently done by assigning a score to each Feature and distributing
//! that score to each input containing that feature. For example, if a
//! thousand inputs all contain the feature F1, then they will all derive
//! a thousandth of F1’s score from it. On the other hand, if only two inputs
//! contain the feature F2, then they will each get half of F2’s score from it.
//! In short, an input’s final score is the sum of the score of each of its
//! features divided by their frequencies.
//!
//! It is not a perfectly fair system because the score of each feature is
//! currently wrong in many cases. For example, a single comparison instruction
//! can currently yield 16 different features for just one input. If that
//! happens, the score of those features will be too high and the input will be
//! over-rated. On the other hand, if it yields only 1 feature, it will be
//! under-rated. My intuition is that all these features could be grouped by
//! the address of their common comparison instruction, and that they should
//! share a common score that increases sub-linearly with the number of
//! features in the group. But it is difficult to implement efficiently.
//!

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;
use std::ops::Range;

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use rand::distributions::uniform::{UniformFloat, UniformSampler};
use rand::distributions::Distribution;

use crate::data_structures::{Slab, SlabKey, WeightedIndex};
use crate::world::{FuzzerEvent, WorldAction};
use crate::{Feature, FuzzedInput, Mutator};

/// Index of an input in the Pool
pub enum PoolIndex<M: Mutator> {
    Normal(SlabKey<Input<M>>),
    Favored,
}

impl<M: Mutator> Clone for PoolIndex<M> {
    fn clone(&self) -> Self {
        match self {
            PoolIndex::Normal(idx) => PoolIndex::Normal(*idx),
            PoolIndex::Favored => PoolIndex::Favored,
        }
    }
}
impl<M: Mutator> Copy for PoolIndex<M> {}

/**
 * An element stored in the pool, containing its value, cache, mutation step,
 * as well as analysed code coverage and computed score.
*/
pub struct Input<M: Mutator> {
    /// The keys of the features for which there are no simpler inputs in the
    /// pool reaching the feature.
    least_complex_for_features: BTreeSet<SlabKey<FeatureInPool<M>>>,
    /// Holds the key of each [FeatureInPool] associated with this input.
    all_features: Vec<SlabKey<FeatureInPool<M>>>,
    /// The computed score of the input
    score: f64,
    /// Data associated with the input: value, cache, and mutation step
    data: FuzzedInput<M>,
    /// Cached complexity of the value.
    ///
    /// It should always be equal to [mutator.complexity(&self.data.value, &self.data.cache)](Mutator::complexity)
    complexity: f64,
    /// The corresponding index of the input in [pool.inputs](self::Pool::inputs)
    idx_in_pool: usize,
}

pub struct FeatureInPool<M: Mutator> {
    pub key: SlabKey<FeatureInPool<M>>, // slab key
    pub(crate) feature: Feature,
    group_key: SlabKey<FeatureGroup>,
    inputs: Vec<SlabKey<Input<M>>>,
    least_complex_input: SlabKey<Input<M>>,
    pub least_complexity: f64,
    old_multiplicity: usize, // cache used when deleting inputs to know how to evolve the score of inputs
}
impl<M: Mutator> Clone for FeatureInPool<M> {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            feature: self.feature,
            group_key: self.group_key,
            inputs: self.inputs.clone(),
            least_complex_input: self.least_complex_input,
            least_complexity: self.least_complexity,
            old_multiplicity: self.old_multiplicity,
        }
    }
}
impl<M: Mutator> PartialEq for FeatureInPool<M> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.feature == other.feature
            && self.inputs == other.inputs
            && self.least_complex_input == other.least_complex_input
            && self.least_complexity == other.least_complexity
            && self.old_multiplicity == other.old_multiplicity
    }
}
impl<M: Mutator> fmt::Debug for FeatureInPool<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Feature {{ {:?}, f: {:#b}, inputs: {:?}, least_cplx: {:.2}, old_mult: {} }}",
            self.key, self.feature.0, self.inputs, self.least_complexity, self.old_multiplicity
        )
    }
}

impl<M: Mutator> FeatureInPool<M> {
    fn new(
        key: SlabKey<Self>,
        feature: Feature,
        group_key: SlabKey<FeatureGroup>,
        inputs: Vec<SlabKey<Input<M>>>,
        least_complex_input: SlabKey<Input<M>>,
        least_complexity: f64,
    ) -> Self {
        let old_multiplicity = inputs.len();
        Self {
            key,
            feature,
            group_key,
            inputs,
            least_complex_input,
            least_complexity,
            old_multiplicity,
        }
    }
}

pub struct FeatureForIteration<M: Mutator> {
    pub key: SlabKey<FeatureInPool<M>>,
    pub(crate) feature: Feature,
}
impl<M: Mutator> Clone for FeatureForIteration<M> {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            feature: self.feature,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct FeatureGroupId {
    id: Feature,
}

impl Feature {
    fn group_id(&self) -> FeatureGroupId {
        FeatureGroupId {
            // erase last 8 bits, which stand for the payload
            id: self.erasing_payload(),
        }
    }
}

pub struct FeatureGroup {
    id: FeatureGroupId, // group represented by the first feature that could belong to the group
    idcs: Range<usize>,
    old_size: usize,
}
impl FeatureGroup {
    fn new(id: FeatureGroupId, idcs: Range<usize>) -> Self {
        let old_size = idcs.end - idcs.start;
        Self { id, idcs, old_size }
    }
    pub fn size(&self) -> usize {
        self.idcs.end - self.idcs.start
    }
}

impl<M: Mutator> Copy for FeatureForIteration<M> {}

pub struct Pool<M: Mutator> {
    pub features: Vec<FeatureForIteration<M>>,
    pub slab_features: Slab<FeatureInPool<M>>,

    feature_groups: Vec<SlabKey<FeatureGroup>>,
    pub slab_feature_groups: Slab<FeatureGroup>,

    inputs: Vec<SlabKey<Input<M>>>,
    slab_inputs: Slab<Input<M>>,

    favored_input: Option<FuzzedInput<M>>,

    pub average_complexity: f64,
    cumulative_weights: Vec<f64>,
    rng: SmallRng,
}

impl<M: Mutator> Pool<M> {
    pub fn default() -> Self {
        Pool {
            features: Vec::new(),
            slab_features: Slab::new(),

            feature_groups: Vec::default(),
            slab_feature_groups: Slab::new(),

            inputs: Vec::default(),
            slab_inputs: Slab::new(),

            favored_input: None,

            average_complexity: 0.0,
            cumulative_weights: Vec::default(),
            rng: SmallRng::from_entropy(),
        }
    }

    pub(crate) fn add_favored_input(&mut self, data: FuzzedInput<M>) {
        self.favored_input = Some(data);
    }

    pub fn score(&self) -> f64 {
        *self.cumulative_weights.last().unwrap_or(&0.0)
    }

    pub(crate) fn add(
        &mut self,
        data: FuzzedInput<M>,
        complexity: f64,
        existing_features: Vec<SlabKey<FeatureInPool<M>>>,
        new_features: Vec<Feature>,
    ) -> Vec<WorldAction<M::Value>> {
        let element_key: SlabKey<Input<M>> = {
            let element = Input {
                least_complex_for_features: BTreeSet::default(),
                all_features: vec![],
                score: 0.0,
                data,
                complexity,
                idx_in_pool: self.inputs.len(),
            };
            let i_key = self.slab_inputs.insert(element);
            self.inputs.push(i_key);

            i_key
        };

        let mut to_delete: Vec<SlabKey<Input<M>>> = vec![];

        for feature_key in existing_features.iter() {
            let feature = &mut self.slab_features[*feature_key];

            for input_key in feature.inputs.iter() {
                let affected_element = &mut self.slab_inputs[*input_key];
                if affected_element.complexity >= complexity {
                    // add (element, feature_key) to list [(Element, [Feature])]
                    // binary search element there, then add feature to the end of it

                    // TODO: change this!
                    // instead, make list of elements to remove feature_key from
                    // and then process them all at once?
                    // and also for each element in this list a list of features to delete
                    affected_element.least_complex_for_features.remove(feature_key);
                    if affected_element.least_complex_for_features.is_empty() {
                        // then this will only be called once by element
                        to_delete.push(*input_key);
                    }
                }
            }
            let element = &mut self.slab_inputs[element_key];

            if feature.least_complexity >= complexity {
                element.least_complex_for_features.insert(*feature_key);
                feature.least_complex_input = element_key;
                feature.least_complexity = complexity;
            }

            element.all_features.push(*feature_key);
            feature.inputs.push(element_key);
        }

        let element = &mut self.slab_inputs[element_key];
        for &f in new_features.iter() {
            let f_key = self.slab_features.next_key();

            let new_feature_for_iter = FeatureForIteration { key: f_key, feature: f };
            let group_key = Self::insert_feature(
                &mut self.features,
                &mut self.feature_groups,
                &mut self.slab_feature_groups,
                new_feature_for_iter,
            );

            let f_in_pool = FeatureInPool::new(f_key, f, group_key, vec![element_key], element_key, complexity);
            self.slab_features.insert(f_in_pool);

            element.all_features.push(f_key);
            element.least_complex_for_features.insert(f_key);
        }

        to_delete.sort();
        to_delete.dedup();

        let deleted_values: Vec<_> = to_delete
            .iter()
            .map(|&key| self.slab_inputs[key].data.value.clone())
            .collect();

        self.delete_elements(to_delete, element_key);

        // iterate over new elements and change score for new group sizes
        let mut new_features_iter = new_features.iter().peekable();

        while let Some(&&next_feature) = new_features_iter.peek() {
            let feature_for_iter_idx = self
                .features
                .binary_search_by_key(&next_feature, |f| f.feature)
                .unwrap();
            let feature_for_iter = &self.features[feature_for_iter_idx];
            let group = {
                let feature_in_pool = &mut self.slab_features[feature_for_iter.key];
                &mut self.slab_feature_groups[feature_in_pool.group_key]
            };

            for f_for_iter in self.features[group.idcs.clone()].iter() {
                let feature_key = f_for_iter.key;
                let feature_in_pool = &mut self.slab_features[feature_key];

                let old_feature_score = Self::score_of_feature(group.old_size, feature_in_pool.old_multiplicity);
                let new_feature_score = Self::score_of_feature(group.size(), feature_in_pool.inputs.len());
                let change_in_score = new_feature_score - old_feature_score;

                for &input_key in feature_in_pool.inputs.iter() {
                    if input_key != element_key {
                        let element_with_feature = &mut self.slab_inputs[input_key];
                        element_with_feature.score += change_in_score;
                    }
                }

                // reset old_multiplicity as it is not needed anymore and will need to be correct
                // for the next call to pool.add
                feature_in_pool.old_multiplicity = feature_in_pool.inputs.len();
            }

            let prev_feature = self.slab_features[feature_for_iter.key].feature;

            while let Some(&&next_feature) = new_features_iter.peek() {
                let feature_for_iter_idx = self
                    .features
                    .binary_search_by_key(&next_feature, |f| f.feature)
                    .unwrap();
                let feature_for_iter = &self.features[feature_for_iter_idx];

                if feature_for_iter.feature.group_id() == prev_feature.group_id() {
                    let _ = new_features_iter.next();
                    continue;
                } else {
                    break;
                }
            }

            group.old_size = group.size();
        }

        for feature_key in existing_features.iter() {
            let feature_in_pool = &mut self.slab_features[*feature_key];

            let group = &self.slab_feature_groups[feature_in_pool.group_key];

            let old_feature_score = Self::score_of_feature(group.old_size, feature_in_pool.old_multiplicity);
            let new_feature_score = Self::score_of_feature(group.size(), feature_in_pool.inputs.len());

            let change_in_score = new_feature_score - old_feature_score;

            for &input_key in feature_in_pool.inputs.iter() {
                if input_key != element_key {
                    let element_with_feature = &mut self.slab_inputs[input_key];
                    element_with_feature.score += change_in_score;
                }
            }
            feature_in_pool.old_multiplicity = feature_in_pool.inputs.len();
        }

        let element = &mut self.slab_inputs[element_key];

        for f_key in element.all_features.iter() {
            let feature_in_pool = &mut self.slab_features[*f_key];
            let group = &self.slab_feature_groups[feature_in_pool.group_key];
            let feature_score = Self::score_of_feature(group.size(), feature_in_pool.inputs.len());
            element.score += feature_score;
        }

        let value = element.data.value.clone();

        let mut actions: Vec<WorldAction<M::Value>> = Vec::new();

        if !deleted_values.is_empty() {
            actions.push(WorldAction::ReportEvent(FuzzerEvent::Replace(deleted_values.len())));
        } else {
            actions.push(WorldAction::ReportEvent(FuzzerEvent::New));
            actions.push(WorldAction::Add(value, vec![]));
        }

        for i in deleted_values.into_iter() {
            actions.push(WorldAction::Remove(i));
        }

        self.update_stats();

        // self.sanity_check();

        actions
    }

    pub fn delete_elements(&mut self, to_delete: Vec<SlabKey<Input<M>>>, should_not_update_key: SlabKey<Input<M>>) {
        for &to_delete_key in to_delete.iter() {
            let to_swap_idx = self.inputs.len() - 1;
            let to_swap_key = *self.inputs.last().unwrap();
            // println!("will delete input with key {}", to_delete_key);
            let to_delete_idx = self.slab_inputs[to_delete_key].idx_in_pool;

            let to_swap_el = &mut self.slab_inputs[to_swap_key];
            to_swap_el.idx_in_pool = to_delete_idx;

            self.inputs.swap(to_delete_idx, to_swap_idx);
            self.inputs.pop();

            let to_delete_el = &mut self.slab_inputs[to_delete_key];
            // to_delete_el.idx_in_pool = to_swap_idx; // not necessary, element will be deleted

            // TODO: not ideal to clone all features
            let all_features = to_delete_el.all_features.clone();

            for f_key in all_features {
                let f_in_pool = &mut self.slab_features[f_key];
                f_in_pool.inputs.remove_item(&to_delete_key); // this updates new multiplicity

                let group = &self.slab_feature_groups[f_in_pool.group_key];

                let new_feature_score = Self::score_of_feature(group.old_size, f_in_pool.inputs.len());
                let old_feature_score = Self::score_of_feature(group.old_size, f_in_pool.old_multiplicity);
                let change_in_score = new_feature_score - old_feature_score;

                for input_key in f_in_pool.inputs.iter() {
                    if *input_key != should_not_update_key {
                        let element_with_feature = &mut self.slab_inputs[*input_key];
                        element_with_feature.score += change_in_score;
                    }
                }
                f_in_pool.old_multiplicity = f_in_pool.inputs.len();
            }
            self.slab_inputs.remove(to_delete_key);
        }
    }

    pub(crate) fn remove_lowest_scoring_input(&mut self) -> Vec<WorldAction<M::Value>> {
        let slab = &self.slab_inputs;
        let pick_key = self
            .inputs
            .iter()
            .min_by(|&&k1, &&k2| slab[k1].score.partial_cmp(&slab[k2].score).unwrap_or(Ordering::Less))
            .copied()
            .unwrap();

        let deleted_value = self.slab_inputs[pick_key].data.value.clone();

        // use MAX to say we do not ignore any element. it is ugly and should be changed
        self.delete_elements(vec![pick_key], SlabKey::invalid());

        let mut actions: Vec<WorldAction<M::Value>> = Vec::new();
        actions.push(WorldAction::ReportEvent(FuzzerEvent::Remove));
        actions.push(WorldAction::Remove(deleted_value));

        self.update_stats();

        actions
    }

    /// Returns the index of the group of the feature
    fn insert_feature(
        features: &mut Vec<FeatureForIteration<M>>,
        feature_groups: &mut Vec<SlabKey<FeatureGroup>>,
        slab_feature_groups: &mut Slab<FeatureGroup>,
        new_feature_for_iter: FeatureForIteration<M>,
    ) -> SlabKey<FeatureGroup> {
        // TODO: CHANGE THIS, too slow
        let insertion_idx = sorted_insert(features, new_feature_for_iter, |other_f| {
            new_feature_for_iter.feature < other_f.feature
        });

        let group_of_new_feature = new_feature_for_iter.feature.group_id();

        let (group_index, group_key) =
            match feature_groups.binary_search_by_key(&group_of_new_feature, |g| slab_feature_groups[*g].id) {
                Ok(group_idx) => {
                    let group_key = feature_groups[group_idx];
                    let group = &mut slab_feature_groups[group_key];
                    if group.idcs.start == insertion_idx + 1 {
                        group.idcs.start -= 1;
                    } else if group.idcs.contains(&insertion_idx) || group.idcs.end == insertion_idx {
                        group.idcs.end += 1;
                    } else {
                        unreachable!();
                    }
                    (group_idx, group_key)
                }
                Err(group_insertion_index) => {
                    let group = FeatureGroup::new(group_of_new_feature, insertion_idx..(insertion_idx + 1));
                    let group_key = slab_feature_groups.insert(group);
                    feature_groups.insert(group_insertion_index, group_key);
                    (group_insertion_index, group_key)
                }
            };

        // update indices of other groups
        for group_key in feature_groups[group_index + 1..].iter_mut() {
            let group = &mut slab_feature_groups[*group_key];
            group.idcs.end += 1;
            group.idcs.start += 1;
        }

        group_key
    }

    pub fn score_of_feature(group_size: usize, exact_feature_multiplicity: usize) -> f64 {
        1.0 / (group_size as f64 * exact_feature_multiplicity as f64)
    }

    /// Returns the index of an interesting input in the pool
    pub fn random_index(&mut self) -> PoolIndex<M> {
        if self.favored_input.is_some() && (self.rng.gen_bool(0.25) || self.inputs.is_empty()) {
            PoolIndex::Favored
        } else {
            let weight_distr = UniformFloat::new(0.0, self.cumulative_weights.last().unwrap_or(&0.0));
            let dist = WeightedIndex {
                cumulative_weights: &self.cumulative_weights,
                weight_distribution: weight_distr,
            };
            let x = dist.sample(&mut self.rng);
            let key = self.inputs[x];
            PoolIndex::Normal(key)
        }
    }

    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// Update global statistics of the pool following a change in its content
    fn update_stats(&mut self) {
        let slab = &self.slab_inputs;
        self.cumulative_weights = self
            .inputs
            .iter()
            .map(|&key| &slab[key])
            .scan(0.0, |state, x| {
                *state += x.score;
                Some(*state)
            })
            .collect();

        self.average_complexity = self
            .inputs
            .iter()
            .map(|&key| &slab[key])
            .fold(0.0, |c, x| c + x.complexity)
            / self.inputs.len() as f64;
    }

    /// Get the input at the given index along with its complexity and the number of mutations tried on this input
    pub(crate) fn get_ref(&self, idx: PoolIndex<M>) -> &'_ FuzzedInput<M> {
        match idx {
            PoolIndex::Normal(key) => &self.slab_inputs[key].data,
            PoolIndex::Favored => self.favored_input.as_ref().unwrap(),
        }
    }
    /// Get the input at the given index along with its complexity and the number of mutations tried on this input
    pub(crate) fn get(&mut self, idx: PoolIndex<M>) -> &'_ mut FuzzedInput<M> {
        match idx {
            PoolIndex::Normal(key) => &mut self.slab_inputs[key].data,
            PoolIndex::Favored => self.favored_input.as_mut().unwrap(),
        }
    }

    pub(crate) fn retrieve_source_input_for_unmutate(&mut self, idx: PoolIndex<M>) -> Option<&'_ mut FuzzedInput<M>> {
        match idx {
            PoolIndex::Normal(key) => self.slab_inputs.get_mut(key).map(|input| &mut input.data),
            PoolIndex::Favored => Some(self.get(idx)),
        }
    }

    #[cfg(test)]
    fn print_recap(&self) {
        println!("recap inputs:");
        for &input_key in self.inputs.iter() {
            let input = &self.slab_inputs[input_key];
            println!(
                "input with key {:?} has cplx {:.2}, score {:.2}, idx {}, and features: {:?}",
                input_key, input.complexity, input.score, input.idx_in_pool, input.all_features
            );
            println!("        and is best for {:?}", input.least_complex_for_features);
        }
        println!("recap features:");
        for &f_iter in self.features.iter() {
            let f_key = f_iter.key;
            let f_in_pool = &self.slab_features[f_key];
            println!("feature {:?}’s inputs: {:?}", f_key, f_in_pool.inputs);
        }
        println!("recap groups:");
        for (i, group_key) in self.feature_groups.iter().enumerate() {
            let group = &self.slab_feature_groups[*group_key];
            let slab = &self.slab_features;
            println!(
                "group {} has features {:?}",
                i,
                self.features[group.idcs.clone()]
                    .iter()
                    .map(|f| &slab[f.key].key)
                    .collect::<Vec<_>>()
            );
        }
        println!("---");
    }

    #[cfg(test)]
    fn sanity_check(&self) {
        let slab = &self.slab_features;

        self.print_recap();

        let fs = self
            .features
            .iter()
            .map(|f_iter| self.slab_features[f_iter.key].feature)
            .collect::<Vec<_>>();
        assert!(fs.is_sorted());

        let slab_groups = &self.slab_feature_groups;
        assert!(self.feature_groups.iter().is_sorted_by_key(|&g| slab_groups[g].id));
        assert!(self
            .feature_groups
            .iter()
            .is_sorted_by_key(|&g| slab_groups[g].idcs.start));
        assert!(self
            .feature_groups
            .iter()
            .is_sorted_by_key(|&g| slab_groups[g].idcs.end));
        assert!(self
            .feature_groups
            .windows(2)
            .all(|gs| slab_groups[gs[0]].idcs.end == slab_groups[gs[1]].idcs.start));
        assert!(slab_groups[*self.feature_groups.last().unwrap()].idcs.end == self.features.len());

        for f_iter in self.features.iter() {
            let f_key = f_iter.key;
            let f_in_pool = &self.slab_features[f_key];
            for input_key in f_in_pool.inputs.iter() {
                let input = &self.slab_inputs[*input_key];
                assert!(input.all_features.contains(&f_key));
            }
        }

        for input_key in self.inputs.iter() {
            let input = &self.slab_inputs[*input_key];
            assert!(input.score > 0.0);
            let expected_input_score = input.all_features.iter().fold(0.0, |c, &fk| {
                let f = &slab[fk];
                let slab_groups = &self.slab_feature_groups;
                let group = self
                    .feature_groups
                    .iter()
                    .map(|&g| &slab_groups[g])
                    .find(|g| g.id == f.feature.group_id())
                    .unwrap();
                c + Self::score_of_feature(group.size(), f.inputs.len())
            });
            assert!(
                (input.score - expected_input_score).abs() < 0.01,
                format!("{:.2} != {:.2}", input.score, expected_input_score)
            );
            assert!(!input.least_complex_for_features.is_empty());

            for f_key in input.least_complex_for_features.iter() {
                let f_in_pool = &self.slab_features[*f_key];
                assert_eq!(f_in_pool.least_complexity, input.complexity);
                assert!(f_in_pool.inputs.contains(&input_key));
                assert!(
                    f_in_pool
                        .inputs
                        .iter()
                        .find(|&&key| self.slab_inputs[key].complexity < input.complexity)
                        == None
                );
            }
        }

        let mut dedupped_inputs = self.inputs.clone();
        dedupped_inputs.sort();
        dedupped_inputs.dedup();
        assert_eq!(dedupped_inputs.len(), self.inputs.len());

        // let mut dedupped_features = self.features.clone();
        // dedupped_features.sort();
        // dedupped_features.dedup();
        // assert_eq!(dedupped_features.len(), self.features.len());

        for g_key in self.feature_groups.iter() {
            let g = &self.slab_feature_groups[*g_key];
            let slab = &self.slab_features;
            assert!(self.features[g.idcs.clone()]
                .iter()
                .map(|f| &slab[f.key])
                .all(|f| f.feature.group_id() == g.id));
        }
    }
}

/// Add the element in the correct place in the sorted vector
fn sorted_insert<T, F>(vec: &mut Vec<T>, element: T, is_before: F) -> usize
where
    F: Fn(&T) -> bool,
{
    let mut insertion = 0;
    for e in vec.iter() {
        if is_before(e) {
            break;
        }
        insertion += 1;
    }
    vec.insert(insertion, element);
    insertion
}

// TODO: include testing the returned WorldAction
// TODO: write unit tests as data, read them from files
// TODO: write tests for adding inputs that are not simplest for any feature but are predicted to have a greater score
#[cfg(test)]
mod tests {
    use super::*;

    fn mock(cplx: f64) -> FuzzedInput<VoidMutator> {
        FuzzedInput::new(cplx, (), ())
    }

    fn edge_f(pc_guard: usize, intensity: u16) -> Feature {
        Feature::edge(pc_guard, intensity)
    }

    type FK = SlabKey<FeatureInPool<VoidMutator>>;

    #[test]
    fn property_test() {
        use rand::seq::IteratorRandom;
        use rand::seq::SliceRandom;
        use std::iter::FromIterator;

        let mut list_features = vec![];
        for i in 0..3 {
            for j in 0..3 {
                list_features.push(edge_f(i, j));
            }
        }

        for _ in 0..1000 {
            let mut new_features = BTreeSet::from_iter(list_features.iter());
            let mut added_features: Vec<FK> = vec![];

            let mut rng = SmallRng::from_entropy();

            let mut pool = Pool::<VoidMutator>::default();

            for i in 0..rng.gen_range(0, 100) {
                let nbr_new_features = if new_features.len() > 0 {
                    rng.gen_range(if i == 0 { 1 } else { 0 }, new_features.len())
                } else {
                    0
                };
                let mut new_features_1 = new_features
                    .iter()
                    .map(|&&f| f)
                    .choose_multiple(&mut rng, nbr_new_features);
                new_features_1.sort();
                for f in new_features_1.iter() {
                    new_features.remove(f);
                }

                let nbr_existing_features = if new_features_1.len() == 0 {
                    if added_features.len() > 1 {
                        rng.gen_range(1, added_features.len())
                    } else {
                        1
                    }
                } else {
                    if added_features.len() > 0 {
                        rng.gen_range(0, added_features.len())
                    } else {
                        0
                    }
                };

                let mut existing_features_1: Vec<FK> = added_features
                    .choose_multiple(&mut rng, nbr_existing_features)
                    .cloned()
                    .collect();
                let slab = &pool.slab_features;
                existing_features_1.sort_by(|&fk1, &fk2| slab[fk1].feature.cmp(&slab[fk2].feature));

                let max_cplx: f64 = if !existing_features_1.is_empty() && new_features_1.is_empty() {
                    existing_features_1
                        .iter()
                        .map(|&f_key| pool.slab_features[f_key].least_complexity)
                        .choose(&mut rng)
                        .unwrap()
                } else {
                    100.0
                };

                if max_cplx == 1.0 {
                    break;
                }

                let cplx1 = rng.gen_range(1.0, max_cplx);
                for _ in 0..new_features_1.len() {
                    added_features.push(added_features.last().map(|x| FK::new(x.key + 1)).unwrap_or(FK::new(0)));
                }

                let prev_score = pool.score();
                // println!("adding input of cplx {:.2} with new features {:?} and existing features {:?}", cplx1, new_features_1, existing_features_1);
                let _ = pool.add(mock(cplx1), cplx1, existing_features_1, new_features_1);
                // pool.print_recap();
                pool.sanity_check();
                assert!(
                    (pool.score() - prev_score) > -0.01,
                    format!("{:.3} > {:.3}", prev_score, pool.score())
                );
            }
            for _ in 0..pool.len() {
                let prev_score = pool.score();
                let _ = pool.remove_lowest_scoring_input();
                pool.sanity_check();
                assert!(
                    (prev_score - pool.score()) > -0.01,
                    format!("{:.3} < {:.3}", prev_score, pool.score())
                );
            }
        }
    }

    // #[test]
    // fn test_features() {
    //     let x1 = Feature::edge(37, 3);
    //     assert_eq!(x1.score(), 1.0);
    //     println!("{:.x}", x1.0);

    //     let x2 = Feature::edge(std::usize::MAX, 255);
    //     assert_eq!(x2.score(), 1.0);
    //     println!("{:.x}", x2.0);

    //     assert!(x1 < x2);

    //     let y1 = Feature::instruction(56, 89, 88);
    //     assert_eq!(y1.score(), 0.1);
    //     println!("{:.x}", y1.0);

    //     assert!(y1 > x1);

    //     let y2 = Feature::instruction(76, 89, 88);
    //     assert_eq!(y2.score(), 0.1);
    //     println!("{:.x}", y2.0);

    //     assert!(y2 > y1);
    // }

    #[derive(Clone, Copy, Debug)]
    pub struct VoidMutator {}

    impl Mutator for VoidMutator {
        type Value = f64;
        type Cache = ();
        type MutationStep = ();
        type UnmutateToken = ();

        fn cache_from_value(&self, _value: &Self::Value) -> Self::Cache {}

        fn mutation_step_from_value(&self, _value: &Self::Value) -> Self::MutationStep {}

        fn arbitrary(&self, _seed: usize, _max_cplx: f64) -> (Self::Value, Self::Cache) {
            (0.0, ())
        }

        fn max_complexity(&self) -> f64 {
            std::f64::INFINITY
        }

        fn min_complexity(&self) -> f64 {
            0.0
        }

        fn complexity(&self, value: &Self::Value, _state: &Self::Cache) -> f64 {
            *value
        }

        fn mutate(
            &self,
            _value: &mut Self::Value,
            _cache: &mut Self::Cache,
            _step: &mut Self::MutationStep,
            _max_cplx: f64,
        ) -> Self::UnmutateToken {
        }

        fn unmutate(&self, _value: &mut Self::Value, _cache: &mut Self::Cache, _t: Self::UnmutateToken) {}
    }
}
