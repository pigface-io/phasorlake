-- ************************************************************ 
-- Title:	PUBLIC_DVD_MTPASA_RESERVELIMIT_SET_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS MTPASA_RESERVELIMIT_SET owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_MTPASA_RESERVELIMIT_SET_201501010000.csv  
APPEND INTO TABLE MTPASA_RESERVELIMIT_SET	
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss", 
VERSION_DATETIME DATE "yyyy/mm/dd hh24:mi:ss", 
RESERVELIMIT_SET_ID CHAR(20), 
DESCRIPTION CHAR(200), 
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
AUTHORISEDBY CHAR(20), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
