//! Things an app hands over to keep (pictures, meshes), each known by a number.
//!
//! Freed slots are reused. A number is its slot (low 24 bits) and how often that slot was given
//! out before (high 8 bits), so a freed number stays invalid after its slot is reused.

/// The slot bits of a number.
const PLACE: u32 = 0x00ff_ffff;

pub(crate) struct Shelf<T> {
    /// Every slot: how often it was given out, and what it holds.
    places: Vec<(u8, Option<T>)>,
    /// The empty slots.
    free: Vec<u32>,
}

impl<T> Default for Shelf<T> {
    fn default() -> Self {
        Shelf { places: Vec::new(), free: Vec::new() }
    }
}

impl<T> Shelf<T> {
    /// Keeps a thing; gives back its number.
    pub fn put(&mut self, thing: T) -> u32 {
        let place = self.free.pop().unwrap_or(self.places.len() as u32);
        if place as usize == self.places.len() {
            self.places.push((0, None));
        }
        let (given, held) = &mut self.places[place as usize];
        *held = Some(thing);
        place | (*given as u32) << 24
    }
    /// The slot a number stands for, if its thing is still there.
    fn place(&self, number: u32) -> Option<usize> {
        let place = (number & PLACE) as usize;
        self.places.get(place).filter(|(given, held)| *given == (number >> 24) as u8 && held.is_some()).map(|_| place)
    }
    pub fn get(&self, number: u32) -> Option<&T> {
        self.place(number).and_then(|place| self.places[place].1.as_ref())
    }
    pub fn get_mut(&mut self, number: u32) -> Option<&mut T> {
        self.place(number).and_then(|place| self.places[place].1.as_mut())
    }
    /// Lets a thing go and hands it back; its number becomes invalid.
    pub fn take(&mut self, number: u32) -> Option<T> {
        let place = self.place(number)?;
        let (given, held) = &mut self.places[place];
        *given = given.wrapping_add(1);
        self.free.push(place as u32);
        held.take()
    }
}

#[cfg(test)]
mod tests {
    use super::Shelf;

    #[test]
    fn a_number_stands_for_its_thing_until_that_is_let_go() {
        let mut shelf = Shelf::default();
        let (first, second) = (shelf.put("first"), shelf.put("second"));
        assert_eq!((shelf.get(first), shelf.get(second)), (Some(&"first"), Some(&"second")));
        assert_eq!(shelf.take(first), Some("first"));
        assert_eq!((shelf.get(first), shelf.get(second)), (None, Some(&"second")));
        assert_eq!(shelf.take(first), None);
        // The next thing reuses the freed slot under a new number; the old number stays
        // invalid.
        let third = shelf.put("third");
        assert!(third != first && third & 0x00ff_ffff == first & 0x00ff_ffff);
        assert_eq!((shelf.get(third), shelf.get(first)), (Some(&"third"), None));
        *shelf.get_mut(third).unwrap() = "changed";
        assert_eq!(shelf.get(third), Some(&"changed"));
        // A number never given is invalid too.
        assert_eq!(shelf.get(77), None);
    }

    #[test]
    fn things_that_come_and_go_take_no_more_places_than_were_ever_held_at_once() {
        let mut shelf = Shelf::default();
        let kept = shelf.put(0);
        for round in 1..2000 {
            let passing = shelf.put(round);
            assert_eq!(shelf.take(passing), Some(round));
        }
        assert_eq!((shelf.places.len(), shelf.get(kept)), (2, Some(&0)));
    }
}
