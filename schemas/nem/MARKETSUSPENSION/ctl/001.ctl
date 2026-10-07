-- ************************************************************ 
-- Title:	PUBLIC_DVD_MARKETSUSPENSION_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS MARKETSUSPENSION owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_MARKETSUSPENSION_201501010000.csv  
APPEND INTO TABLE MARKETSUSPENSION  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
SUSPENSIONID CHAR(10), 
STARTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
STARTPERIOD FLOAT EXTERNAL, 
ENDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
ENDPERIOD FLOAT EXTERNAL, 
REASON CHAR(64), 
STARTAUTHORISEDBY CHAR(15), 
ENDAUTHORISEDBY CHAR(15), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
