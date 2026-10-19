// Executor falso para los tests de cu-exec: registra llamadas y devuelve respuestas guionadas.
use crate::Executor;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

type Reply = Result<(Value, bool), String>;

#[derive(Default)]
pub struct FakeExec {
    pub calls: RefCell<Vec<(String, Value)>>,
    queue: RefCell<HashMap<String, VecDeque<Reply>>>,
    pub elements: RefCell<Vec<Value>>,
    pub active: RefCell<Option<Value>>,
}

impl FakeExec {
    // Respuestas en orden para un tool; sin guion, responde ok.
    pub fn script(&self, tool: &str, replies: Vec<Reply>) {
        self.queue
            .borrow_mut()
            .entry(tool.to_string())
            .or_default()
            .extend(replies);
    }
}

impl Executor for FakeExec {
    fn call(&self, tool: &str, args: &Value) -> Result<(Value, bool), String> {
        self.calls
            .borrow_mut()
            .push((tool.to_string(), args.clone()));
        let next = self
            .queue
            .borrow_mut()
            .get_mut(tool)
            .and_then(VecDeque::pop_front);
        next.unwrap_or_else(|| Ok((json!({"ok": true}), false)))
    }

    fn collect_elements(&self, _target: &Value) -> Vec<Value> {
        self.elements.borrow().clone()
    }

    fn active_window(&self) -> Option<Value> {
        self.active.borrow().clone()
    }
}
