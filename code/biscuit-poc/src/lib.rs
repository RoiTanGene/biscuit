extern crate datalog_with_constraints as datalog;
extern crate biscuit_vrf as vrf;
extern crate rand;
extern crate curve25519_dalek;
extern crate serde;
extern crate serde_cbor;
#[macro_use]
extern crate nom;

use serde::{Serialize, Deserialize};
use datalog::*;

mod ser;

pub fn default_symbol_table() -> SymbolTable {
  let mut syms = SymbolTable::new();
  syms.insert("authority");
  syms.insert("ambient");

  syms
}

pub struct BiscuitLogic {
  authority: Block,
  blocks: Vec<Block>,
  symbols: SymbolTable,
}

impl BiscuitLogic {
  pub fn new(authority: Block, blocks: Vec<Block>) -> BiscuitLogic {
    println!("creating BiscuitLogic");
    // generate the complete symbol table
    let mut symbols = default_symbol_table();
    symbols.symbols.extend(authority.symbols.symbols.iter().cloned());
    println!("symbol table is now: {:#?}", symbols);

    for block in blocks.iter() {
      symbols.symbols.extend(block.symbols.symbols.iter().cloned());
      println!("symbol table is now: {:#?}", symbols);
    }

    BiscuitLogic { authority, blocks, symbols }
  }

  pub fn check(&self, mut ambient_facts: Vec<Fact>, mut ambient_rules: Vec<Rule>) -> Result<(), Vec<String>> {
    let mut world = World::new();

    let authority_index = self.symbols.get("authority").unwrap();
    let ambient_index = self.symbols.get("ambient").unwrap();

    for fact in self.authority.facts.iter().cloned() {
      if fact.0.ids[0] != ID::Symbol(authority_index) {
        return Err(vec![format!("invalid authority fact: {}", self.symbols.print_fact(&fact))]);
      }

      world.facts.insert(fact);
    }

    // autority caveats are actually rules
    for rule in self.authority.caveats.iter().cloned() {
      world.rules.push(rule);
    }

    world.run();

    if world.facts.iter().find(|fact| fact.0.ids[0] != ID::Symbol(authority_index)).is_some() {
      return Err(vec![String::from("generated authority facts should have the authority context")]);
    }

    //remove authority rules: we cannot create facts anymore in authority scope
    //w.rules.clear();

    for fact in ambient_facts.drain(..) {
      if fact.0.ids[0] != ID::Symbol(ambient_index) {
        return Err(vec![format!("invalid ambient fact: {}", self.symbols.print_fact(&fact))]);
      }

      world.facts.insert(fact);
    }

    for rule in ambient_rules.iter().cloned() {
      world.rules.push(rule);
    }

    world.run();

    // we only keep the verifier rules
    world.rules = ambient_rules;

    let mut errors = vec![];
    for (i, block) in self.blocks.iter().enumerate() {
      let w = world.clone();

      match block.check(i, w, &self.symbols) {
        Err(mut e) => {
          errors.extend(e.drain(..));
        },
        Ok(_) => {}
      }
    }

    if errors.is_empty() {
      Ok(())
    } else {
      Err(errors)
    }
  }

  pub fn create_block(&self) -> Block {
    Block::new((1 + self.blocks.len()) as u32, self.symbols.clone())
  }

  pub fn adjust_authority_symbols(block: &mut Block) {
    let base_symbols = default_symbol_table();

    let new_syms = block.symbols.symbols.split_off(base_symbols.symbols.len());

    block.symbols.symbols = new_syms;
  }

  pub fn adjust_block_symbols(&self, block: &mut Block) {
    let new_syms = block.symbols.symbols.split_off(self.symbols.symbols.len());
    println!("adjusting block symbols from {:#?}\nto {:#?}", block.symbols, new_syms);

    block.symbols.symbols = new_syms;
  }
}

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Block {
  pub index: u32,
  pub symbols: SymbolTable,
  pub facts: Vec<Fact>,
  pub caveats: Vec<Rule>,
}

impl Block {
  pub fn new(index: u32, base_symbols: SymbolTable) -> Block {
    Block {
      index,
      symbols: base_symbols,
      facts: vec![],
      caveats: vec![],
    }
  }

  pub fn symbol_add(&mut self, s: &str) -> ID {
    self.symbols.add(s)
  }

  pub fn symbol_insert(&mut self, s: &str) -> u64 {
    self.symbols.insert(s)
  }

  pub fn check(&self, i: usize, mut world: World, symbols: &SymbolTable) -> Result<(), Vec<String>> {
    let authority_index = symbols.get("authority").unwrap();
    let ambient_index = symbols.get("ambient").unwrap();

    for fact in self.facts.iter().cloned() {
      if fact.0.ids[0] == ID::Symbol(authority_index) ||
        fact.0.ids[0] == ID::Symbol(ambient_index) {
        return Err(vec![format!("Block {}: invalid fact: {}", i, symbols.print_fact(&fact))]);
      }

      world.facts.insert(fact);
    }

    world.run();

    let mut errors = vec![];
    for (j, caveat) in self.caveats.iter().enumerate() {
      let res = world.query_rule(caveat.clone());
      if res.is_empty() {
        errors.push(format!("Block {}: caveat {} failed: {}", i, j, symbols.print_rule(caveat)));
      }
    }

    if errors.is_empty() {
      Ok(())
    } else {
      Err(errors)
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use rand::prelude::*;
  use crate::ser::SerializedBiscuit;
  use nom::HexDisplay;
  use vrf::KeyPair;

  #[test]
  fn basic() {
    let mut rng: StdRng = SeedableRng::seed_from_u64(0);

    let symbols = default_symbol_table();
    let mut authority_block = Block::new(0, symbols);

    let authority = authority_block.symbols.add("authority");
    let ambient = authority_block.symbols.add("ambient");
    let file1 = authority_block.symbols.add("file1");
    let file2 = authority_block.symbols.add("file2");
    let read = authority_block.symbols.add("read");
    let write = authority_block.symbols.add("write");
    let right = authority_block.symbols.insert("right");
    println!("authority symbols: {:#?}", authority_block.symbols);
    authority_block.facts = vec![
      fact(right, &[&authority, &file1, &read]),
      fact(right, &[&authority, &file2, &read]),
      fact(right, &[&authority, &file1, &write]),
    ];

    BiscuitLogic::adjust_authority_symbols(&mut authority_block);

    let root = KeyPair::new(&mut rng);

    let biscuit1 = SerializedBiscuit::new(&root, &authority_block);
    let serialized1 = biscuit1.to_vec();

    println!("generated biscuit token: {} bytes:\n{}", serialized1.len(), serialized1.to_hex(16));

    let biscuit1_deser = SerializedBiscuit::from(&serialized1, root.public).unwrap();
    let biscuit1_logic = biscuit1_deser.deserialize_logic().unwrap();

    // new caveat: can only have read access1
    let mut block2 = biscuit1_logic.create_block();
    let resource = block2.symbols.insert("resource");
    let operation = block2.symbols.insert("operation");
    let caveat1 = block2.symbols.insert("caveat1");

    block2.caveats.push(rule(caveat1, &[var("X")], &[
      pred(resource, &[&ambient, &var("X")]),
      pred(operation, &[&ambient, &read]),
      pred(right, &[&authority, &var("X"), &read])
    ]));

    biscuit1_logic.adjust_block_symbols(&mut block2);

    let keypair2 = KeyPair::new(&mut rng);
    let biscuit2 = biscuit1_deser.append(&keypair2, &block2);

    let serialized2 = biscuit2.to_vec();

    println!("generated biscuit token 2: {} bytes\n{}", serialized2.len(), serialized2.to_hex(16));

    let biscuit2_deser = SerializedBiscuit::from(&serialized2, root.public).unwrap();
    let biscuit2_logic = biscuit2_deser.deserialize_logic().unwrap();

    // new caveat: can only access file1
    let mut block3 = biscuit2_logic.create_block();
    let caveat2 = block3.symbols.insert("caveat2");

    block3.caveats.push(rule(caveat2, &[&file1], &[
      pred(resource, &[&ambient, &file1])
    ]));

    biscuit2_logic.adjust_block_symbols(&mut block3);

    let keypair3 = KeyPair::new(&mut rng);
    let biscuit3 = biscuit2_deser.append(&keypair3, &block3);

    let serialized3 = biscuit3.to_vec();

    println!("generated biscuit token 3: {} bytes\n{}", serialized3.len(), serialized3.to_hex(16));


    let final_token = SerializedBiscuit::from(&serialized3, root.public).unwrap();

    let final_token_logic = final_token.deserialize_logic().unwrap();

    {
      let ambient_facts = vec![
        fact(resource, &[&ambient, &file1]),
        fact(operation, &[&ambient, &read]),
      ];
      let ambient_rules = vec![];

      let res = final_token_logic.check(ambient_facts, ambient_rules);
      println!("res1: {:?}", res);
      res.unwrap();
    }

    {
      let ambient_facts = vec![
        fact(resource, &[&ambient, &file2]),
        fact(operation, &[&ambient, &write]),
      ];
      let ambient_rules = vec![];

      let res = final_token_logic.check(ambient_facts, ambient_rules);
      println!("res2: {:#?}", res);
      res.unwrap();
    }

    panic!()
    /*
    let ambient_facts = vec![
      fact(resource, &[&ambient, &file1]),
      fact(operation, &[&ambient, &read]),
    ];
    let ambient_rules = vec![];

    bench.iter(|| {
      let w = World::biscuit_create(&mut syms, authority_facts.clone(), authority_rules.clone(),
        ambient_facts.clone(), ambient_rules.clone());

      let res = w.query_rule(rule(caveat1, &[var("X")], &[
        pred(resource, &[&ambient, &var("X")]),
        pred(operation, &[&ambient, &read]),
        pred(right, &[&authority, &var("X"), &read])
      ]));

      assert!(!res.is_empty());

      let res = w.query_rule(rule(caveat2, &[&file1], &[
        pred(resource, &[&ambient, &file1])
      ]));

      assert!(!res.is_empty());
    });
    */
  }
}
