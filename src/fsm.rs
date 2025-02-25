#[derive(Clone, Copy)]

pub enum SessionMode {
    /// Peer hosted, hybrid server-client session
    CreateServer,

    ConnectAsClientOnly,
}

pub enum State {
    Menu,
    Connecting {
        server_address: String,
        session_mode: SessionMode,
    },

    Playing,
    Disconnected,
    QuitDialog,
    Quit,
}

pub struct StateMachine {
    state_stack: Vec<State>,
}

impl StateMachine {
    pub fn new() -> Self {
        Self {
            state_stack: Vec::new(),
        }
    }

    pub fn push(&mut self, state: State) {
        self.state_stack.push(state);
    }

    pub fn pop(&mut self) {
        self.state_stack.pop();
    }

    pub fn change(&mut self, state: State) {
        self.state_stack.clear();
        self.push(state);
    }

    pub fn peek(&self) -> Option<&State> {
        self.state_stack.last()
    }

    pub fn peek_mut(&mut self) -> Option<&mut State> {
        self.state_stack.last_mut()
    }
}
