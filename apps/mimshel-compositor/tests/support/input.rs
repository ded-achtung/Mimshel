use smithay::backend::{input::*, winit::WinitVirtualDevice};
pub struct TestInput;
pub struct Button(pub ButtonState);
impl InputBackend for TestInput {
    type Device = WinitVirtualDevice;
    type KeyboardKeyEvent = UnusedEvent;
    type PointerAxisEvent = UnusedEvent;
    type PointerButtonEvent = Button;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = UnusedEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = ();
}
impl Event<TestInput> for Button {
    fn time(&self) -> u64 {
        1_000
    }
    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}
impl PointerButtonEvent<TestInput> for Button {
    fn button_code(&self) -> u32 {
        0x110
    }
    fn state(&self) -> ButtonState {
        self.0
    }
}
pub fn click(state: &mut mimshel_compositor::state::State) {
    for value in [ButtonState::Pressed, ButtonState::Released] {
        state.input::<TestInput>(
            InputEvent::PointerButton {
                event: Button(value),
            },
            (1280, 800).into(),
        );
    }
}
