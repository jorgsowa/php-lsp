===request===
references include_declaration
===file===
<?php
class AppState {
    public static bool $initia$0lized = false;
    //                  ^^^^^^^^^^^ def
}

function bootstrap(): void {
    if (!AppState::$initialized) {
    //              ^^^^^^^^^^^ ref
        setup();
        AppState::$initialized = true;
        //         ^^^^^^^^^^^ ref
    }
}
