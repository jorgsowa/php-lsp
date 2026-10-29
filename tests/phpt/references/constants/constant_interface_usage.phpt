===request===
references include_declaration
===file===
<?php
interface HttpMethods {
    const GET$0_TIMEOUT = 30;
    //    ^^^^^^^^^^^ def
    const POST_TIMEOUT = 60;
}

class Client implements HttpMethods {
    public function request(): void {
        $timeout = self::GET_TIMEOUT;
        //               ^^^^^^^^^^^ ref
    }
}

echo HttpMethods::GET_TIMEOUT;
//                ^^^^^^^^^^^ ref
