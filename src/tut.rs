let x; // Declare a variable without initializing it
x=42; 
let y = 32;
let z = 43;
let a:i32 = 32; // Type implementation
let _= 43; // Thow away, if a fn then throw away it's result and don't worn about it.
let pair = ('a', 21);
pair.0; // Access 'a'
pair.1; // Access 21
let pair2 (mychar, someint) = ('a', 21); // Tuple destructuring
assert!(mychar, 'a');
assert!(someint, 21);
let (_, right) = split.split_at(middle); // Ignore the left part of the tuple