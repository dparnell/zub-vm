use std::collections::HashMap;

use fnv::FnvBuildHasher;

use super::*;

use std::mem;

const STACK_SIZE:  usize = 4096;
const HEAP_GROWTH: usize = 2;

const GC_TRIGGER_COUNT: usize = 1024;

pub struct CallFrame {
    closure: Handle<Object>,
    ip: usize,
    stack_start: usize,
}

impl CallFrame {
    pub fn new(closure: Handle<Object>, stack_start: usize) -> Self {
        CallFrame {
            closure,
            ip: 0,
            stack_start,
        }
    }

    pub fn read_byte(&mut self) -> u8 {
        let ip = self.ip;
        self.ip += 1;
        self.with_chunk(|c| c.read_byte(ip))
    }

    pub fn read_u16(&mut self) -> u16 {
        let ip = self.ip;
        self.ip += 2;
        self.with_chunk(|c| c.read_u16(ip))
    }

    pub fn read_u64(&mut self) -> u64 {
        let ip = self.ip;
        self.ip += 8;
        self.with_chunk(|c| c.read_u64(ip))
    }

    pub fn read_constant_at(&mut self, idx: u8) -> Value {
        self.with_chunk(|c| *c.get_constant(idx).expect("invalid constant index"))
    }

    pub fn read_constant(&mut self) -> Value {
        let idx = self.read_byte();
        self.read_constant_at(idx)
    }

    pub fn with_chunk<F, T>(&self, fun: F) -> T
        where
            F: FnOnce(&Chunk) -> T
    {
        unsafe {
            let closure = self.closure.get_unchecked()
                .as_closure()
                .expect("closure reference by construction");
            fun(closure.chunk())
        }
    }
}

macro_rules! binary_op {
    ($self:ident, $op:tt) => {
        let b = $self.pop();
        let a = $self.pop();

        if let (Variant::Float(a), Variant::Float(b)) = (a.decode(), b.decode()) {
            let c = a $op b;
            $self.push(c.into());

            return
        }

        // TODO: ERROR HERE
    }
}

pub struct VM {
    heap: Heap<Object>,
    next_gc: usize,

    pub globals: HashMap<String, Value, FnvBuildHasher>,
    pub open_upvalues: Vec<UpValue>,

    pub stack: Vec<Value>,
    frames: Vec<CallFrame>,
}

impl VM {
    pub fn new() -> Self {
        VM {
            stack:   Vec::with_capacity(STACK_SIZE),
            heap:    Heap::default(),
            next_gc: GC_TRIGGER_COUNT,
            globals: HashMap::with_hasher(FnvBuildHasher::default()),
            frames:  Vec::with_capacity(256),
            open_upvalues: Vec::with_capacity(16)
        }
    }

    pub fn exec(&mut self, atoms: &[ExprNode]) {
        let function = {
            let compiler = Compiler::new(&mut self.heap);
            compiler.compile(atoms)
        };

        let closure = Closure::new(function, Vec::new());
        let value = self.allocate(Object::Closure(closure)).into();

        self.push(value);
        self.call(0);

        self.run()
    }

    pub fn add_native(&mut self, name: &str, func: fn(&Heap<Object>, &[Value]) -> Value, arity: u8) {
        let function = self.allocate(
            Object::native_fn(name, arity, func)
        );

        self.globals.insert(name.into(), function.into());
    }

    fn run(&mut self)  {
        while !self.frames.is_empty() {
            let inst = self.read_byte();
            decode_op!(inst, self)
        }
    }

    #[inline]
    fn call_closure(&mut self, handle: Handle<Object>, arity: u8) {
        let closure = self.deref(handle)
            .as_closure()
            .expect("redundant cast to succeed");

        let last = self.stack.len();
        let frame_start = last - (arity + 1) as usize;

        if closure.arity() != arity {
            self.runtime_error(&format!("arity mismatch: {} != {}", closure.arity(), arity))
        }

        let frame = CallFrame::new(handle, frame_start);
        self.frames.push(frame);
    }

    #[inline]
    fn closure(&mut self) {
        let value = self.frame_mut().read_constant();
        let function = value.as_object()
            .map(|o| self.deref(o))
            .and_then(|o| o.as_function())
            .cloned()
            .expect("closure expected function argument");
        
        let mut upvalues = Vec::new();

        for _ in 0 .. function.upvalue_count() {
            let is_local = self.read_byte() > 0;
            let idx = self.read_byte() as usize;
            let upvalue = if is_local {
                self.capture_upvalue(idx)
            } else {
                self.current_closure().get(idx)
            };

            upvalues.push(upvalue)
        }

        let closure = Closure::new(function, upvalues);
        let value = self.allocate(Object::Closure(closure)).into();

        self.push(value)
    }

    #[inline]
    fn call(&mut self, arity: u8) {
        // TODO: MAKE OPTION FOR INLINING HERE!

        let last = self.stack.len();
        let frame_start = last - (arity + 1) as usize;
        let callee = self.stack[frame_start].decode();

        if let Variant::Obj(handle) = callee {
            use self::Object::*;

            match unsafe { self.heap.get_unchecked(handle) } {
                Closure(_) => {
                    self.call_closure(handle, arity)
                },
                NativeFunction(ref native) => {
                    if native.arity != arity {
                        self.runtime_error(&format!("arity mismatch: {} != {}", native.arity, arity))
                    }

                    let value = (native.function)(&self.heap, &self.stack[frame_start..]);

                    self.stack.pop();
                    self.stack.push(value);
                },

                _ => self.runtime_error("bad call")
            }
        }
    }

    #[inline]
    fn ret(&mut self) {
        if let Some(frame) = self.frames.pop() {
            let return_value = self.pop();

            if frame.stack_start < self.stack.len() {
                self.close_upvalues(frame.stack_start)
            }
            
            self.stack.truncate(frame.stack_start);
            self.push(return_value);
        } else {
            self.runtime_error("can't return from top-level");
        }
    }

    #[inline]
    fn capture_upvalue(&mut self, idx: usize) -> UpValue {
        let offset = self.frame().stack_start + idx;

        self.open_upvalues.iter().rev()
            .find(|&up| {
                up.as_local().map(|i| i == offset).unwrap_or(false)
            })
            .cloned()
            .unwrap_or_else(|| {
                let up = UpValue::new(offset);
                self.open_upvalues.push(up.clone());
                up
            })
    }

    fn current_closure(&mut self) -> &mut Closure {
        let handle = self.frame_mut().closure;
        self.deref_mut(handle)
            .as_closure_mut()
            .expect("valid closure")
    }

    #[inline]
    fn set_upvalue(&mut self) {
        let value = self.peek();
        let idx = self.frame_mut().read_byte();
        let closure = self.current_closure();
        let res = closure.get(idx as usize).set(value);

        if let Err(i) = res {
            self.stack[i] = value
        }
    }

    #[inline]
    fn get_upvalue(&mut self) {
        let idx = self.frame_mut().read_byte();
        let value = self.current_closure()
            .get(idx as usize)
            .get()
            .unwrap_or_else(|i| self.stack[i]);
        
        self.push(value)
    }

    #[inline]
    fn close_upvalue(&mut self) {
        let end = self.stack.len() - 1;

        self.close_upvalues(end);
        self.pop();
    }

    #[inline]
    fn close_upvalues(&mut self, stack_end: usize) {
        let mut open_upvalues = Vec::new();

        mem::swap(&mut self.open_upvalues, &mut open_upvalues);

        for mut up in open_upvalues {
            if up.get().map_err(|i| i >= stack_end).is_err() {
                up.close(|i| self.stack[i]);
                
                self.open_upvalues.push(up)
            }
        }
    }

    #[inline]
    fn allocate(&mut self, object: Object) -> Handle<Object> {
        let handle = self.heap.insert(object).into_handle();

        if self.heap.len() * mem::size_of::<Object>() >= self.next_gc {
            self.next_gc *= HEAP_GROWTH;

            let upvalue_iter = self.open_upvalues.iter()
                .flat_map(|u| u.get().ok())
                .flat_map(|v| v.as_object());

            let globals_iter = self.globals.values().flat_map(Value::as_object);
            let stack_iter = self.stack.iter().flat_map(Value::as_object);

            let exclude = stack_iter
                .chain(Some(handle))
                .chain(globals_iter)
                .chain(upvalue_iter);
            
            self.heap.clean_excluding(exclude);
        }

        handle
    }

    fn constant(&mut self, idx: u8) {
        let val = self.frame_mut().read_constant_at(idx);
        self.push(val)
    }

    #[inline]
    fn print(&mut self) {
        let value = self.pop();
        println!("{}", value.with_heap(&self.heap))
    }

    #[inline]
    fn add(&mut self) {
        let a = self.pop();
        let b = self.pop();

        use self::Variant::*;

        match (a.decode(), b.decode()) {
            (Float(a), Float(b)) => return self.push((a + b).into()),
            _ => {}
        }
    }

    #[inline]
    fn get_global(&mut self) {
        let global = self.frame_mut()
            .read_constant()
            .as_object()
            .map(|o| self.deref(o))
            .and_then(|o| o.as_string())
            .expect("`GetGlobal` requires a string identifier");
        
        if let Some(value) = self.globals.get(global).cloned() {
            self.push(value)
        } else {
            self.runtime_error(&format!("undefined global variable: `{}`", global.clone()))
        }
    }

    #[inline]
    fn define_global(&mut self) {
        let var = self.frame_mut().read_constant()
            .as_object()
            .map(|o| self.deref(o))
            .and_then(|o| o.as_string())
            .cloned()
            .expect("expected constant to be a string value");
        
        let lhs = self.stack.pop().unwrap();

        self.globals.insert(var, lhs);
    }

    #[inline]
    fn set_global(&mut self) {
        let handle = self.frame_mut().read_constant()
            .as_object()
            .filter(|&o| self.deref(o).as_string().is_some())
            .expect("expected constant to be a string value");
    
        let var = unsafe {
            handle.get_mut_unchecked()
                .as_string()
                .unwrap()
        };

        let value = *self.stack.last().unwrap();

        if let Some(slot) = self.globals.get_mut(var) {
            *slot = value
        } else {
            self.globals.insert(var.clone(), value);
        }
    }

    #[inline]
    fn list(&mut self) {
        let element_count = self.read_byte();

        let mut content = Vec::new();

        for _ in 0 .. element_count {
            content.push(self.pop())
        }

        let val = self.allocate(Object::List(List::new(content))).into();
        self.push(val)
    }

    #[inline]
    fn set_element(&mut self) {
        let list = self.pop();
        let idx  = if let Variant::Float(ref index) = self.pop().decode() {
            *index as usize
        } else {
            panic!("Can't index list with non-number")
        };

        let value = self.pop();

        let list_object = list
            .as_object()
            .map(|o| self.heap.get_mut_unchecked(o));

        if let Some(Object::List(ref mut list)) = list_object {
            list.set(idx as usize, value)
        }
    }

    #[inline]
    fn get_element(&mut self) {
        let list = self.pop();
        let idx  = if let Variant::Float(ref index) = self.pop().decode() {
            *index as usize
        } else {
            panic!("Can't index list with non-number")
        };

        let list_handle = list
            .as_object()
            .unwrap();

        let list = self.deref(list_handle);

        let element = list.as_list().unwrap().get(idx as usize);

        self.push(element)
    }

    fn runtime_error(&self, err: &str) {
        eprintln!("[error]: {}.", err);
        for frame in self.frames.iter().rev() {
            let ip = frame.ip;
            frame.with_chunk(|chunk| {
                let name = chunk.name();
                let line = chunk.line(ip);
                eprintln!("         at [line {}] in {}", line, name);
            });
        }
        ::std::process::exit(1);
    }

    fn on_loop(&mut self) {
        self.frame_mut().ip -= self.read_u16() as usize
    }

    fn get_local(&mut self) {
        let start = self.frame().stack_start;
        let idx = self.read_byte() as usize;
        let val = self.stack[start + idx];

        self.push(val)
    }

    fn set_local(&mut self) {
        let val = self.peek();
        let start = self.frame().stack_start;
        let idx = self.read_byte() as usize;

        self.stack[start + idx] = val
    }

    fn immediate(&mut self) {
        let raw = self.frame_mut().read_u64();
        let val = unsafe { Value::from_raw(raw) };

        self.push(val)
    }

    fn imm_nil(&mut self) {
        self.push(Value::nil());
    }

    fn imm_true(&mut self) {
        self.push(Value::truelit());
    }

    fn imm_false(&mut self) {
        self.push(Value::falselit());
    }

    #[inline]
    fn sub(&mut self) {
        binary_op!(self, -);
    }

    #[inline]
    fn mul(&mut self) {
        binary_op!(self, *);
    }

    #[inline]
    fn div(&mut self) {
        binary_op!(self, /);
    }

    #[inline]
    fn neg(&mut self) {
        if let Variant::Float(a) = self.pop().decode() {
            self.push((-a).into());
        }
    }

    #[inline]
    fn not(&mut self) {
        let a = self.pop();

        self.push(
            if a.truthy() {
                Value::falselit()
            } else {
                Value::truelit()
            }
        )
    }

    #[inline]
    fn eq(&mut self) {
        binary_op!(self, ==);
    }

    #[inline]
    fn gt(&mut self) {
        binary_op!(self, >);
    }

    #[inline]
    fn lt(&mut self) {
        binary_op!(self, <);
    }

    #[inline]
    fn jmp(&mut self) {
        self.frame_mut().ip = self.read_u16() as usize
    }

    #[inline]
    fn jze(&mut self) {
        let ip = self.read_u16();
        if !self.peek().truthy() {
            self.frame_mut().ip = ip as usize
        }
    }

    #[inline]
    fn op_loop(&mut self) {
        self.frame_mut().ip -= self.read_u16() as usize
    }

    fn frame(&self) -> &CallFrame {
        self.frames.last().expect("frames to be nonempty")
    }

    fn frame_mut(&mut self) -> &mut CallFrame {
        self.frames.last_mut().expect("frames to be nonempty")
    }

    #[inline]
    fn read_byte(&mut self) -> u8 {
        self.frame_mut().read_byte()
    }

    #[inline]
    fn read_u16(&mut self) -> u16 {
        self.frame_mut().read_u16()
    }

    fn push(&mut self, value: Value) {
        if self.stack.len() == STACK_SIZE {
            panic!("STACK OVERFLOW >:(");
        }

        self.stack.push(value);
    }

    #[inline]
    fn pop(&mut self) -> Value {
        self.stack.pop().expect("stack to be nonempty")
    }

    #[inline]
    fn peek(&mut self) -> Value {
        *self.stack.last().expect("stack to be nonempty")
    }

    #[inline]
    fn deref(&self, o: Handle<Object>) -> &Object {
        unsafe { self.heap.get_unchecked(o) }
    }

    #[inline]
    fn deref_mut(&mut self, o: Handle<Object>) -> &mut Object {
        self.heap.get_mut_unchecked(o)
    }
}