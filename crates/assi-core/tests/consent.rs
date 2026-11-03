use assi_core::consent::{ConsentError, Permissions, Session, State};
fn requested() -> Permissions {
    Permissions {
        view: true,
        control: true,
        files: false,
    }
}
#[test]
fn control_requires_consent_and_ready_transport_and_stops_on_close() {
    let mut s = Session::new(requested());
    assert!(!s.allows_control());
    assert_eq!(s.activate(), Err(ConsentError::InvalidTransition));
    s.accept(requested()).unwrap();
    assert!(!s.allows_control());
    s.activate().unwrap();
    assert!(s.allows_control());
    s.close();
    assert!(!s.allows_control());
    assert_eq!(s.activate(), Err(ConsentError::InvalidTransition));
    assert_eq!(s.accept(requested()), Err(ConsentError::InvalidTransition));
    assert_eq!(s.state(), State::Closed);
}
#[test]
fn consent_cannot_expand_permissions() {
    let mut s = Session::new(requested());
    assert_eq!(
        s.accept(Permissions {
            files: true,
            ..requested()
        }),
        Err(ConsentError::InvalidPermissions)
    );
    assert_eq!(
        s.accept(Permissions {
            view: false,
            ..requested()
        }),
        Err(ConsentError::InvalidPermissions)
    );
    assert_eq!(s.state(), State::Pending);
    s.accept(Permissions {
        control: false,
        ..requested()
    })
    .unwrap();
    s.activate().unwrap();
    assert!(!s.allows_control());
}
