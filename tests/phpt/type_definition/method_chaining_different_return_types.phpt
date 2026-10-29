===request===
type_definition
===file===
<?php
class Request {
//    ^^^^^^^ type
    public function withHeader(string $name): self { return $this; }
}
class Response {
    public function withStatus(int $code): self { return $this; }
}
class Client {
    public function request(): Request { return new Request(); }
    public function response(): Response { return new Response(); }
}
$c = new Client();
$r = $c->request()->withHeader('auth')$0->send();
