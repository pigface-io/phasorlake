-- ************************************************************ 
-- Title:	PUBLIC_DVD_RESIDUE_TRK_201503.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS RESIDUE_TRK owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_RESIDUE_TRK_201503010000.csv  
APPEND INTO TABLE RESIDUE_TRK  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
CONTRACTID CHAR(30), 
VERSIONNO FLOAT EXTERNAL, 
RUNDATE DATE "yyyy/mm/dd hh24:mi:ss", 
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
AUTHORISEDBY CHAR(15), 
POSTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
POSTEDBY CHAR(15), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
STATUS CHAR(15), 
AUCTIONID CHAR(30)) 
-- End of Script 
