-- ************************************************************ 
-- Title:	PUBLIC_DVD_RESIDUE_CONTRACTS_201503.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS RESIDUE_CONTRACTS owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_RESIDUE_CONTRACTS_201503010000.csv  
APPEND INTO TABLE RESIDUE_CONTRACTS  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
CONTRACTYEAR FLOAT EXTERNAL, 
QUARTER FLOAT EXTERNAL, 
TRANCHE FLOAT EXTERNAL, 
CONTRACTID CHAR(30), 
STARTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
ENDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
NOTIFYDATE DATE "yyyy/mm/dd hh24:mi:ss", 
AUCTIONDATE DATE "yyyy/mm/dd hh24:mi:ss", 
CALCMETHOD CHAR(20), 
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
AUTHORISEDBY CHAR(15), 
NOTIFYPOSTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
NOTIFYBY CHAR(15), 
POSTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
POSTEDBY CHAR(15), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
DESCRIPTION CHAR(80), 
AUCTIONID CHAR(30)) 
-- End of Script 
