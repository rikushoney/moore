package pkg is

	type BIT is ('0','1');

	procedure P1;
	procedure "and"; -- should fail

	function F1 return BIT;
	function "and" return BIT;

end;

library work;
use work.pkg.all;
entity foo is end;

architecture bar of foo is
	-- Currently the architecture is required to trigger typeck of the entire
	-- library.
begin end;