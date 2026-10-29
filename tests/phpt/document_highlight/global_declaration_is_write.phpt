===description===
`global $x` is a write (it binds the global into the local scope) and must
be highlighted as WRITE, not READ.
===request===
document_highlight
===file===
<?php
function foo(): void {
    global $co$0nfig;
//         ^^^^^^^ write
    echo $config['debug'];
//       ^^^^^^^ read
}
