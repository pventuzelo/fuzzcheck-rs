use std::collections::HashMap;

use rand::rngs::ThreadRng;
use rand::Rng;

use rand::distributions::uniform::{UniformFloat, UniformSampler};
use rand::distributions::Distribution;

use crate::input::FuzzerInput;
use crate::weighted_index::WeightedIndex;
use crate::world::FuzzerEvent;
use crate::world::FuzzerWorld;

// TODO: think through derive
#[derive(PartialEq, Eq, Hash, Clone)]
pub enum Feature {
    Edge(EdgeFeature),
    Comparison(ComparisonFeature),
}

#[derive(PartialEq, Eq, Hash, Clone)]
pub struct EdgeFeature {
    pc_guard: usize,
    intensity: u8,
}

fn score_from_counter(counter: u16) -> u8 {
    if counter == core::u16::MAX {
        16
    } else if counter <= 3 {
        counter as u8
    } else {
        (16 - counter.leading_zeros() + 1) as u8
    }
}

impl EdgeFeature {
    pub fn new(pc_guard: usize, counter: u16) -> EdgeFeature {
        EdgeFeature {
            pc_guard,
            intensity: score_from_counter(counter),
        }
    }
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, PartialOrd, Ord)]
pub struct ComparisonFeature {
    pc: usize,
    id: u8,
}

impl ComparisonFeature {
    pub fn new(pc: usize, arg1: u64, arg2: u64) -> ComparisonFeature {
        ComparisonFeature {
            pc,
            id: score_from_counter(arg1.wrapping_sub(arg2).count_ones() as u16),
        }
    }
    /*
    init(pc: UInt, arg1: UInt64, arg2: UInt64) {
            self.init(pc: pc, argxordist: scoreFromCounter(UInt8((arg1 &- arg2).nonzeroBitCount)))
        }
    */
}

impl Feature {
    fn score(&self) -> f64 {
        match self {
            Feature::Edge(_) => 1.0,
            Feature::Comparison(_) => 0.5,
        }
    }
}

pub enum InputPoolIndex {
    Normal(usize),
    Favored,
}

pub struct FuzzerState<Input> {
    input: Input,
}

#[derive(Clone)]
pub struct InputPoolElement<Input: Clone> {
    pub input: Input,
    pub complexity: f64,
    features: Vec<Feature>,
    score: f64,
    flagged_for_deletion: bool,
}

// TODO: think of req for Input
impl<Input: FuzzerInput> InputPoolElement<Input> {
    pub fn new(input: Input, complexity: f64, features: Vec<Feature>) -> InputPoolElement<Input> {
        InputPoolElement {
            input,
            complexity,
            features,
            score: -1.0,
            flagged_for_deletion: false,
        }
    }
}

pub struct InputPool<Input: FuzzerInput> {
    pub inputs: Vec<InputPoolElement<Input>>,
    favored_input: Option<InputPoolElement<Input>>,
    cumulative_weights: Vec<f64>,
    pub score: f64,
    pub smallest_input_complexity_for_feature: HashMap<Feature, f64>,
}

impl<Input: FuzzerInput> InputPool<Input> {
    pub fn new() -> InputPool<Input> {
        InputPool {
            inputs: vec![],
            favored_input: None,
            cumulative_weights: vec![],
            score: 0.0,
            smallest_input_complexity_for_feature: HashMap::new(),
        }
    }

    pub fn get(&self, idx: InputPoolIndex) -> &InputPoolElement<Input> {
        match idx {
            InputPoolIndex::Normal(idx) => &self.inputs[idx],
            InputPoolIndex::Favored => &self.favored_input.as_ref().unwrap(),
        }
    }
    fn set(&mut self, idx: InputPoolIndex, element: InputPoolElement<Input>) {
        match idx {
            InputPoolIndex::Normal(idx) => self.inputs[idx] = element,
            InputPoolIndex::Favored => panic!("Cannot change the favored input"),
        }
    }

    fn complexity_ratio(simplest: f64, other: f64) -> f64 {
        let square = |x| x * x;
        square(simplest / other)
    }

    fn update_scores<W>(&mut self) -> impl FnOnce(&mut W) -> ()
    where
        W: FuzzerWorld<Input = Input>,
    {
        let mut sum_cplx_ratios: HashMap<Feature, f64> = HashMap::new();
        for input in self.inputs.iter_mut() {
            input.flagged_for_deletion = true;
            input.score = 0.0;
            for f in input.features.iter() {
                let simplest_cplx = self.smallest_input_complexity_for_feature[f];
                let ratio = Self::complexity_ratio(simplest_cplx, input.complexity);
                assert!(ratio <= 1.0);
                if (simplest_cplx - input.complexity).abs() < std::f64::EPSILON {
                    input.flagged_for_deletion = false;
                }
            }
            if input.flagged_for_deletion {
                continue;
            }
            for f in input.features.iter() {
                let simplest_cplx = self.smallest_input_complexity_for_feature[f];
                let ratio = Self::complexity_ratio(simplest_cplx, input.complexity);
                *sum_cplx_ratios.entry(f.clone()).or_insert(0.0) += ratio;
            }
        }

        for input in self.inputs.iter_mut() {
            if input.flagged_for_deletion {
                continue;
            }
            for f in input.features.iter() {
                let simplest_cplx = self.smallest_input_complexity_for_feature[f];
                let sum_ratios = sum_cplx_ratios[f];
                let base_score = f.score() / sum_ratios;
                let ratio = Self::complexity_ratio(simplest_cplx, input.complexity);
                let score = base_score * ratio;
                input.score += score;
            }
        }

        let inputs_to_delete: Vec<Input> = self
            .inputs
            .iter()
            .filter_map(|i| {
                if i.flagged_for_deletion {
                    None
                } else {
                    Some(i.input.clone())
                }
            })
            .collect();

        let _ = self.inputs.drain_filter(|i| i.flagged_for_deletion);
        self.score = self.inputs.iter().fold(0.0, |x, next| x + next.score);
        let deleted_some = !inputs_to_delete.is_empty();
        move |w| {
            //for i in inputs_to_delete {
            // w.remove_from_output_corpus(i);
            //}
            if deleted_some {
                w.report_event(FuzzerEvent::Deleted(inputs_to_delete.len()), Option::None);
            }
        }
    }

    pub fn add<W>(&mut self, elements: Vec<InputPoolElement<Input>>) -> impl FnOnce(&mut W) -> ()
    where
        W: FuzzerWorld<Input = Input>,
    {
        for element in elements.iter() {
            for f in element.features.iter() {
                let complexity = self.smallest_input_complexity_for_feature.get(&f);
                if complexity == Option::None || element.complexity < *complexity.unwrap() {
                    let _ = self
                        .smallest_input_complexity_for_feature
                        .insert(f.clone(), element.complexity);
                }
            }
            self.inputs.push(element.clone());
        }
        let world_update_1 = self.update_scores();

        self.cumulative_weights = self
            .inputs
            .iter()
            .scan(0.0, |state, x| {
                *state += x.score;
                Some(*state)
            })
            .collect();

        |w: &mut W| {
            world_update_1(w);
            for _ in elements {}
        }
    }
    fn add_one<W>(&mut self, element: InputPoolElement<Input>) -> impl FnOnce(&mut W) -> ()
    where
        W: FuzzerWorld<Input = Input>,
    {
        for f in element.features.iter() {
            let complexity = self.smallest_input_complexity_for_feature.get(&f);
            if complexity == Option::None || element.complexity < *complexity.unwrap() {
                let _ = self
                    .smallest_input_complexity_for_feature
                    .insert(f.clone(), element.complexity);
            }
        }
        self.inputs.push(element.clone());
        let world_update_1 = self.update_scores();

        self.cumulative_weights = self
            .inputs
            .iter()
            .scan(0.0, |state, x| {
                *state += x.score;
                Some(*state)
            })
            .collect();

        |w: &mut W| {
            world_update_1(w);
            //w.add_to_output_corpus(element.input);
        }
    }

    pub fn random_index(&self, rand: &mut ThreadRng) -> InputPoolIndex {
        if self.favored_input.is_some() && (rand.gen_bool(0.25) || self.inputs.is_empty()) {
            InputPoolIndex::Favored
        } else {
            let weight_distr = UniformFloat::new(0.0, self.cumulative_weights.last().unwrap_or(&0.0));
            let dist = WeightedIndex {
                cumulative_weights: self.cumulative_weights.clone(),
                weight_distribution: weight_distr,
            };
            let x = dist.sample(rand);
            InputPoolIndex::Normal(x)
        }
    }

    fn empty(&mut self) {
        self.inputs.clear();
        self.score = 0.0;
        self.cumulative_weights.clear();
        self.smallest_input_complexity_for_feature.clear();
    }
}