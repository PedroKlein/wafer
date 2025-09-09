(module
  ;; Define a function that adds two i32 numbers
  (func $add (param $a i32) (param $b i32) (result i32)
    local.get $a
    local.get $b
    i32.add
  )
  
  ;; Define a function that multiplies two i32 numbers
  (func $multiply (param $a i32) (param $b i32) (result i32)
    local.get $a
    local.get $b
    i32.mul
  )
  
  ;; Export the functions so they can be called from the host
  (export "add" (func $add))
  (export "multiply" (func $multiply))
)
