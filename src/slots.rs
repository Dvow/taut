//! Things an app hands over to keep (pictures, meshes), each known by a number.
//!
//! Freed slots are reused. A number is its slot (low 24 bits) and how often that slot was given
//! out before (high 8 bits, its generation), so a freed number stays invalid after its slot is
//! reused.

/// The slot bits of a number.
const SLOT_BITS: u32 = 0x00ff_ffff;

pub(crate) struct Slots<T> {
    /// Every slot: its generation, and what it holds.
    slots: Vec<(u8, Option<T>)>,
    /// The empty slots.
    free: Vec<u32>,
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Slots { slots: Vec::new(), free: Vec::new() }
    }
}

impl<T> Slots<T> {
    /// Keeps a thing; gives back its number.
    pub fn put(&mut self, thing: T) -> u32 {
        let slot = self.free.pop().unwrap_or(self.slots.len() as u32);
        if slot as usize == self.slots.len() {
            self.slots.push((0, None));
        }
        let (generation, value) = &mut self.slots[slot as usize];
        *value = Some(thing);
        slot | (*generation as u32) << 24
    }
    /// The slot of a number, if its thing is still there.
    fn slot_of(&self, number: u32) -> Option<usize> {
        let slot = (number & SLOT_BITS) as usize;
        self.slots.get(slot).filter(|(generation, value)| *generation == (number >> 24) as u8 && value.is_some()).map(|_| slot)
    }
    pub fn get(&self, number: u32) -> Option<&T> {
        self.slot_of(number).and_then(|slot| self.slots[slot].1.as_ref())
    }
    pub fn get_mut(&mut self, number: u32) -> Option<&mut T> {
        self.slot_of(number).and_then(|slot| self.slots[slot].1.as_mut())
    }
    /// Lets a thing go and hands it back; its number becomes invalid.
    pub fn take(&mut self, number: u32) -> Option<T> {
        let slot = self.slot_of(number)?;
        let (generation, value) = &mut self.slots[slot];
        *generation = generation.wrapping_add(1);
        self.free.push(slot as u32);
        value.take()
    }
}

#[cfg(test)]
mod tests {
    use super::Slots;

    #[test]
    fn a_number_stands_for_its_thing_until_that_is_let_go() {
        let mut slots = Slots::default();
        let (first, second) = (slots.put("first"), slots.put("second"));
        assert_eq!((slots.get(first), slots.get(second)), (Some(&"first"), Some(&"second")));
        assert_eq!(slots.take(first), Some("first"));
        assert_eq!((slots.get(first), slots.get(second)), (None, Some(&"second")));
        assert_eq!(slots.take(first), None);
        // The next thing reuses the freed slot under a new number; the old number stays
        // invalid.
        let third = slots.put("third");
        assert!(third != first && third & 0x00ff_ffff == first & 0x00ff_ffff);
        assert_eq!((slots.get(third), slots.get(first)), (Some(&"third"), None));
        *slots.get_mut(third).unwrap() = "changed";
        assert_eq!(slots.get(third), Some(&"changed"));
        // A number never given is invalid too.
        assert_eq!(slots.get(77), None);
    }

    #[test]
    fn things_that_come_and_go_take_no_more_places_than_were_ever_held_at_once() {
        let mut slots = Slots::default();
        let kept = slots.put(0);
        for round in 1..2000 {
            let passing = slots.put(round);
            assert_eq!(slots.take(passing), Some(round));
        }
        assert_eq!((slots.slots.len(), slots.get(kept)), (2, Some(&0)));
    }
}
