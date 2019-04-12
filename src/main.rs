#![feature(const_fn)]
#![feature(const_str_as_bytes)]

use crossbeam_channel::{Sender, Receiver, bounded};
use serde_json;
use serde::Deserialize;
use std::thread;
use std::sync::RwLock;
use std::sync::Arc;
use std::time::Duration;
use std::collections::{HashSet, HashMap};
use std::convert::From;
use std::fs::File;
use std::io::BufReader;
use lazy_static::lazy_static;

#[derive(Deserialize)]
struct RawInput {
    plain: String,
    cipher: String,
    spigots: Option<usize>,
}

struct Input {
    plain: Vec<u8>,
    cipher: Vec<u8>,
    spigots: Option<usize>,
}

impl From<RawInput> for Input {
    fn from(raw: RawInput) -> Input {
        let mut input = Input {
            plain: raw.plain.into_bytes(),
            cipher: raw.cipher.into_bytes(),
            spigots: raw.spigots,
        };

        for c in input.plain.iter_mut() {
            *c -= 97;
        }

        for c in input.cipher.iter_mut() {
            *c -= 97;
        }

        input
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rotor {
    mapper: [u8; 26],
    rev_mapper: [u8; 26],
    notch: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Reflector([u8; 26]);

impl Rotor {
    fn from(mapper: &str, notch: usize) -> Rotor {
        let mut result = Rotor {
            mapper: [0; 26],
            rev_mapper: [0; 26],
            notch,
        };

        let bytes = mapper.as_bytes();
        for i in 0..26 {
            result.mapper[i] = bytes[i] - 65;
        }

        for i in 0..26 {
            result.rev_mapper[result.mapper[i] as usize] = i as u8;
        }

        result
    }
}

impl Reflector {
    fn from(mapper: &str) -> Reflector {
        let mut result = Reflector(Default::default());

        let bytes = mapper.as_bytes();
        for i in 0..26 {
            result.0[i] = bytes[i] - 65;
        }

        result
    }
}

lazy_static! {
    static ref ROTORS: [Rotor; 5] = [
        Rotor::from("EKMFLGDQVZNTOWYHXUSPAIBRCJ", 16),
        Rotor::from("AJDKSIRUXBLHWTMCQGZNPYFVOE", 4),
        Rotor::from("BDFHJLCPRTXVZNYEIWGAKMUSQO", 21),
        Rotor::from("ESOVPZJAYQUIRHXLNFTGKDCMWB", 9),
        Rotor::from("VZBRGITYUPSDNHLXAWMJQOFECK", 25),
    ];
    static ref REFLECTOR: Reflector = Reflector::from("YRUHQSLDPXNGOKMIEBFZCWVJAT");
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RotorConfig {
    rotor: &'static Rotor,
    offset: usize,
}

impl RotorConfig {
    fn initial(rotor: &'static Rotor) -> Self {
        RotorConfig {
            rotor,
            offset: 0,
        }
    }

    fn next(&self) -> (RotorConfig, bool) {
        let up = self.rotor.notch == self.offset;

        let mut result = self.clone();
        result.offset = (self.offset + 1) % 26;
        (result, up)
    }

    fn map(&self, input: u8) -> u8 {
        let shifted = (self.offset + input as usize) % 26;
        ((self.rotor.mapper[shifted] + 26) - self.offset as u8) % 26
    }

    fn rev_map(&self, input: u8) -> u8 {
        let shifted = (self.offset + input as usize) % 26;
        ((self.rotor.rev_mapper[shifted] + 26) - self.offset as u8) % 26
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RotorSet {
    rotors: [RotorConfig; 3],
    reflector: &'static Reflector,
}

impl RotorSet {
    fn initial(rotors: [&'static Rotor; 3], reflector: &'static Reflector) -> Self {
        let [a, b, c] = rotors;
        RotorSet {
            rotors: [
                RotorConfig::initial(a),
                RotorConfig::initial(b),
                RotorConfig::initial(c),
            ],
            reflector,
        }
    }

    fn next(&self) -> (RotorSet, bool) {
        let mut result = self.clone();
        let mut advancing = true;
        for index in 0..3 {
            if !advancing { break; }
            let (nrotor, nadvancing) = result.rotors[index].next();

            result.rotors[index] = nrotor;
            advancing = nadvancing;
        }

        (result, advancing)
    }

    fn advance(&self, i: usize) -> RotorSet {
        if i == 0 { return self.clone(); }

        let mut result = self.next().0;
        for _ in 1..i {
            result = result.next().0;
        }

        result
    }

    fn map(&self, input: u8) -> u8 {
        let a1 = self.rotors[0].map(input);
        let a2 = self.rotors[1].map(a1);
        let a3 = self.rotors[2].map(a2);

        let b0 = self.reflector.0[a3 as usize];
        let b1 = self.rotors[2].rev_map(b0);
        let b2 = self.rotors[1].rev_map(b1);
        self.rotors[0].rev_map(b2)
    }
}

fn kick_dispatcher(tx: Sender<RotorSet>, result: Arc<RwLock<Option<(RotorSet, HashMap<u8, u8>)>>>) {
    thread::spawn(move || {
        let mut counter = 0;
        let mut milestone = 1000;
        for a in 0..5 {
            for b in 0..5 {
                if a == b { continue; }

                for c in 0..5 {
                    if a == c || b == c { continue; }

                    let initial = RotorSet::initial([ &ROTORS[a], &ROTORS[b], &ROTORS[c] ], &*REFLECTOR);
                    let mut set = initial.clone();
                    counter+=1;

                    loop {
                        if result.read().unwrap().is_some() { return; }

                        match tx.send_timeout(set.clone(), Duration::from_secs(1)) {
                            Err(_) => continue,
                            Ok(()) => {
                                let (nset, _) = set.next();
                                if nset == initial { break; }
                                set = nset;
                                counter+=1;
                            }
                        }
                    }

                    if counter > milestone {
                        println!("{} / {}", counter, milestone);
                        while counter > milestone { milestone += 1000; }
                    }
                }
            }
        }

        println!("Finished: {}", counter);
    }).join();
}

fn boot_checker(rx: Receiver<RotorSet>, result: Arc<RwLock<Option<(RotorSet, HashMap<u8, u8>)>>>, input: Arc<Input>) {
    thread::spawn(move || {
        let mut crossed = HashSet::<(u8, u8)>::new();
        let mut searching = HashMap::<u8, u8>::new();
        let mut accessed = Vec::<bool>::with_capacity(input.plain.len());

        loop {
            crossed.clear();
            let init = match rx.recv() {
                Ok(c) => c,
                Err(_) => return,
            };

            let rotors = init.next().0;

            // Initial guess
            let first = input.plain[0];
            for mapped in 0..26 {
                if crossed.contains(&(first, mapped)) { continue; }

                searching.clear();
                accessed.clear();
                accessed.resize(input.plain.len(), false);

                searching.insert(first, mapped);
                searching.insert(mapped, first);

                let rotor_out = rotors.map(mapped);
                let mapped_out = input.cipher[0];

                let rotor_other = searching.get(&rotor_out);
                let mapped_other = searching.get(&mapped_out);

                if rotor_other.is_some() && rotor_other != Some(&mapped_out) ||
                    mapped_other.is_some() && mapped_other != Some(&rotor_out) {
                        continue;
                    }

                searching.insert(rotor_out, mapped_out);
                searching.insert(mapped_out, rotor_out);

                accessed[0] = true;

                let mut flag = true;
                'outer: while flag {
                    for i in 0..input.plain.len() {
                        if accessed[i] { continue; }

                        if let Some(mapped) = searching.get(&input.plain[i]) {
                            let current_rotors = rotors.advance(i);
                            let rotor_out = current_rotors.map(*mapped);
                            let mapped_out = input.cipher[i];

                            if crossed.contains(&(rotor_out, mapped_out)) {
                                for (k, v) in searching.iter() {
                                    crossed.insert((*k, *v));
                                }
                                flag = false;
                                break 'outer;
                            }

                            let rotor_other = searching.get(&rotor_out);
                            let mapped_other = searching.get(&mapped_out);

                            if rotor_other.is_some() && rotor_other != Some(&mapped_out) ||
                                mapped_other.is_some() && mapped_other != Some(&rotor_out) {
                                    flag = false;
                                    for (k, v) in searching.iter() {
                                        crossed.insert((*k, *v));
                                    }
                                    crossed.insert((rotor_out, mapped_out));
                                    crossed.insert((mapped_out, rotor_out));
                                    break 'outer;
                                }

                            searching.insert(rotor_out, mapped_out);
                            searching.insert(mapped_out, rotor_out);
                        } else if let Some(mapped) = searching.get(&input.cipher[i]) {
                            let current_rotors = rotors.advance(i);
                            let rotor_out = current_rotors.map(*mapped);
                            let mapped_out = input.plain[i];

                            if crossed.contains(&(rotor_out, mapped_out)) {
                                for (k, v) in searching.iter() {
                                    crossed.insert((*k, *v));
                                }
                                flag = false;
                                break 'outer;
                            }

                            let rotor_other = searching.get(&rotor_out);
                            let mapped_other = searching.get(&mapped_out);

                            if rotor_other.is_some() && rotor_other != Some(&mapped_out) ||
                                mapped_other.is_some() && mapped_other != Some(&rotor_out) {
                                    flag = false;
                                    for (k, v) in searching.iter() {
                                        crossed.insert((*k, *v));
                                    }
                                    crossed.insert((rotor_out, mapped_out));
                                    crossed.insert((mapped_out, rotor_out));
                                    break 'outer;
                                }

                            searching.insert(rotor_out, mapped_out);
                            searching.insert(mapped_out, rotor_out);
                        } else {
                            // Go directly to the next character
                            continue;
                        }

                        accessed[i] = true;
                        continue 'outer;
                    }

                    // All elements are verified or not mapped
                    break;
                }

                if flag {
                    println!("FOUND");
                    *result.write().unwrap() = Some((init, searching));
                    return;
                }
            }
        }
    });
}

fn main() {
    let input_file = File::open("./input.json").unwrap();
    let reader = BufReader::new(input_file);
    let input: RawInput = serde_json::from_reader(reader).unwrap();

    let shared: Arc<Input> = Arc::new(input.into());

    let (tx, rx) = bounded(0);

    let result: Arc<RwLock<_>> = Default::default();

    for _ in 0..4 {
        boot_checker(rx.clone(), result.clone(), shared.clone());
    }

    kick_dispatcher(tx, result.clone());

    // Here we have a output
    let guard = result.read().unwrap();
    let (ref rotors, ref mapper) = *guard.as_ref().unwrap();

    println!("From right to left: ");
    for r in rotors.rotors.iter() {
        for i in 0..5 {
            if r.rotor == &ROTORS[i] {
                println!("Rotor {}", i + 1);
            }
        }
        println!("  Position {}", (65 + r.offset as u8) as char);
    }

    println!("From right to left: ");
    for (k, v) in mapper.iter() {
        if k >= v { continue; }
        println!("  {} <-> {}", (65 + k) as char, (65 + v) as char);
    }
}
